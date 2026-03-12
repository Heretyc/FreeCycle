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
    agent_server, autostart, config, gpu_monitor, lockfile, logging, ollama, tray, AppState,
};
use std::sync::Arc;
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

    // Initialize logging (verbose writes to ~/freecycle-verbose.log)
    let _guard = logging::init_logging(cli.verbose).context("Failed to initialize logging")?;

    info!("FreeCycle v{} starting", env!("CARGO_PKG_VERSION"));

    // Check for existing instance via lockfile
    let lock = lockfile::ProcessLock::acquire().context("Failed to check process lock")?;
    if lock.is_none() {
        info!("Another instance of FreeCycle is already running. Exiting quietly.");
        return Ok(());
    }
    let _lock = lock.unwrap();
    info!("Process lock acquired");

    // Load configuration
    let config = config::FreeCycleConfig::load_or_create_default()
        .context("Failed to load configuration")?;
    info!(
        "Configuration loaded from {}",
        config::config_path().display()
    );

    // Disable Ollama auto-start (registry Run key and scheduled tasks)
    if let Err(e) = autostart::disable_ollama_autostart() {
        warn!("Failed to disable Ollama auto-start: {}", e);
    }

    // Register FreeCycle to auto-start with Windows
    if let Err(e) = autostart::register_freecycle_autostart() {
        warn!("Failed to register FreeCycle auto-start: {}", e);
    }

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

    // Run the tray icon on the main thread (required by Windows message pump)
    // This blocks until the user exits via the tray context menu
    info!("Starting system tray interface");
    tray::run_tray(Arc::clone(&shared_state), shutdown_tx, &runtime)?;

    info!("FreeCycle shutting down");
    Ok(())
}
