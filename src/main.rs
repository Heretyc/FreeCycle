//! FreeCycle: GPU-aware Ollama lifecycle manager for Windows 11.
//!
//! Runs as a system tray application that monitors GPU usage and game processes,
//! automatically enabling/disabling networked Ollama access when the GPU is available
//! for LLM compute workloads.

#[cfg(not(windows))]
compile_error!("FreeCycle only supports Windows");

use anyhow::{Context, Result};
use clap::Parser;
use freecycle::{
    agent_server, autostart, config, exposure_monitor, gpu_monitor, lockfile, logging,
    model_catalog, ollama, security, tray, AppState,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, RwLock};
use tracing::{error, info, warn};

/// FreeCycle: GPU-aware Ollama lifecycle manager.
///
/// Monitors for games and GPU-intensive programs, automatically enabling/disabling
/// networked Ollama access when the GPU is available for LLM compute.
#[derive(Parser, Debug)]
#[command(name = "freecycle", version, about)]
struct Cli {
    /// Enable verbose debug logging to ~/freecycle-verbose.log
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Install the rustls crypto provider before any TLS usage
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls CryptoProvider");

    // Initialize logging (verbose writes to ~/freecycle-verbose.log)
    let _guard = logging::init_logging(cli.verbose).context("Failed to initialize logging")?;

    info!("FreeCycle v{} starting", env!("CARGO_PKG_VERSION"));

    // Check for existing instance via lockfile
    let lock = lockfile::ProcessLock::acquire().context("Failed to check process lock")?;
    if lock.is_none() {
        info!("Another instance of FreeCycle is already running. Exiting quietly.");
        return Ok(());
    }
    let lock = Arc::new(lock.unwrap());
    info!("Process lock acquired");

    // Load configuration
    let mut config = config::FreeCycleConfig::load_or_create_default()
        .context("Failed to load configuration")?;
    info!(
        "Configuration loaded from {}",
        config::config_path().display()
    );

    // Kill all Ollama processes including the tray app (ollama app.exe) so
    // we can safely modify its settings database before it relaunches.
    ollama::kill_existing_ollama();

    // Set expose=0 in Ollama's SQLite settings so the tray app won't
    // re-expose Ollama on 0.0.0.0 if it ever relaunches the server.
    if let Err(e) = ollama::disable_ollama_network_exposure() {
        warn!("Failed to disable Ollama network exposure setting: {}", e);
    }

    // Disable Ollama auto-start (registry Run key and scheduled tasks)
    if let Err(e) = autostart::disable_ollama_autostart() {
        warn!("Failed to disable Ollama auto-start: {}", e);
    }

    // Lock OLLAMA_HOST to 127.0.0.1 in the user environment registry so
    // Ollama's tray or any other launcher always starts it on localhost.
    if let Err(e) = autostart::enforce_ollama_localhost_binding(&config.ollama.secure_host, config.ollama.port) {
        warn!("Failed to enforce Ollama localhost binding: {}", e);
    }

    // Register FreeCycle to auto-start with Windows
    if let Err(e) = autostart::register_freecycle_autostart() {
        warn!("Failed to register FreeCycle auto-start: {}", e);
    }

    // Ensure Ed25519 keypair exists for secure mode
    match security::ensure_keypair(&config.security) {
        Ok(_key) => info!("Ed25519 keypair ready"),
        Err(e) => warn!("Failed to ensure Ed25519 keypair: {}", e),
    }

    // Ensure TLS certificate exists for secure mode
    let cert_regenerated = match security::ensure_tls_cert(&config.security) {
        Ok(regenerated) => {
            if regenerated {
                info!("TLS certificate regenerated");
            } else {
                info!("TLS certificate ready");
            }
            regenerated
        }
        Err(e) => {
            warn!("Failed to ensure TLS certificate: {}", e);
            false
        }
    };

    // Ensure server UUID exists for secure mode
    match security::ensure_identity_uuid(&mut config) {
        Ok(uuid) => info!("Server UUID ready: {}", uuid),
        Err(e) => warn!("Failed to ensure server UUID: {}", e),
    }

    // Build GPU fingerprint for server identity
    let fingerprint = security::build_gpu_fingerprint(&config.security);
    info!("GPU fingerprint: {}", fingerprint);

    // Check if Ollama is installed
    if !ollama::is_ollama_installed(&config) {
        error!(
            "Ollama is not installed. Please download it from https://ollama.ai and install it, \
             then restart FreeCycle."
        );
    }

    // Build the async runtime
    let runtime = tokio::runtime::Runtime::new().context("Failed to create Tokio runtime")?;

    // Create shared state
    let shared_state = Arc::new(RwLock::new(AppState::new(config)));

    // Create a shutdown signal channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Spawn the GPU monitor (5-second interval)
    let state_clone = Arc::clone(&shared_state);
    let shutdown_rx_gpu = shutdown_rx.clone();
    runtime.spawn(async move {
        gpu_monitor::run_gpu_monitor(state_clone, shutdown_rx_gpu).await;
    });

    // Spawn the Ollama lifecycle manager
    let state_clone = Arc::clone(&shared_state);
    let shutdown_rx_ollama = shutdown_rx.clone();
    runtime.spawn(async move {
        ollama::run_ollama_manager(state_clone, shutdown_rx_ollama).await;
    });

    // Spawn the agent signal HTTP server
    let state_clone = Arc::clone(&shared_state);
    let shutdown_rx_agent = shutdown_rx.clone();
    runtime.spawn(async move {
        if let Err(e) = agent_server::run_agent_server(state_clone, shutdown_rx_agent).await {
            error!("Agent signal server error: {}", e);
        }
    });

    // Spawn the model download/update manager
    let state_clone = Arc::clone(&shared_state);
    let shutdown_rx_models = shutdown_rx.clone();
    runtime.spawn(async move {
        ollama::run_model_manager(state_clone, shutdown_rx_models).await;
    });

    // Spawn the Ollama exposure monitor (will check compatibility_mode internally)
    let state_clone = Arc::clone(&shared_state);
    let shutdown_rx_exposure = shutdown_rx.clone();
    runtime.spawn(async move {
        exposure_monitor::run_exposure_monitor(state_clone, shutdown_rx_exposure).await;
    });

    // Spawn the model catalog updater
    let state_clone = Arc::clone(&shared_state);
    let shutdown_rx_catalog = shutdown_rx.clone();
    runtime.spawn(async move {
        model_catalog::run_catalog_updater(state_clone, shutdown_rx_catalog).await;
    });

    // Spawn background lock refresh task (every 10s, well within 30s expiry)
    // This keeps the lock alive even if the tray message loop is temporarily frozen
    // (e.g. during system sleep/resume or modal dialogs).
    let lock_for_refresh = Arc::clone(&lock);
    let shutdown_rx_lock = shutdown_rx.clone();
    runtime.spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        let mut shutdown = shutdown_rx_lock;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = lock_for_refresh.refresh() {
                        warn!("Failed to refresh process lock: {}", e);
                    }
                }
                _ = shutdown.changed() => break,
            }
        }
    });

    // Run the tray icon on the main thread (required by Windows message pump)
    // This blocks until the user exits via the tray context menu
    info!("Starting system tray interface");
    tray::run_tray(
        Arc::clone(&shared_state),
        shutdown_tx,
        &runtime,
        &*lock,
        cert_regenerated,
    )?;

    info!("FreeCycle shutting down");
    Ok(())
}
