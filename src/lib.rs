//! FreeCycle shared library surface.
//!
//! Exposes the application modules and shared state used by the Windows tray
//! binary and by integration tests that exercise the HTTP API.

#[cfg(not(windows))]
compile_error!("FreeCycle only supports Windows");

pub mod agent_server;
pub mod autostart;
pub mod config;
pub mod gpu_monitor;
pub mod lockfile;
pub mod logging;
pub mod ollama;
pub mod state;
pub mod tray;

use std::sync::Arc;
use tokio::sync::RwLock;

/// Top-level shared application state accessible by all subsystems.
///
/// Wrapped in `Arc<RwLock<>>` for safe concurrent access from the GPU monitor,
/// tray publisher, agent server, and Ollama manager.
pub struct AppState {
    /// Current application state machine status.
    pub status: state::FreeCycleStatus,

    /// Loaded configuration.
    pub config: config::FreeCycleConfig,

    /// Information about the currently active agent task, if any.
    pub agent_task: Option<state::AgentTask>,

    /// Current in-memory operator override from the tray menu.
    pub manual_override: Option<state::ManualOverride>,

    /// Timestamp when a blacklisted process was last detected.
    pub last_blacklist_seen: Option<std::time::Instant>,

    /// Timestamp when VRAM usage last dropped below the idle threshold (300MB).
    pub vram_idle_since: Option<std::time::Instant>,

    /// Deadline until which Ollama stays stopped after system resume.
    pub wake_block_until: Option<std::time::Instant>,

    /// Whether Ollama is currently running.
    pub ollama_running: bool,

    /// Current VRAM usage in bytes.
    pub vram_used_bytes: u64,

    /// Total VRAM available in bytes.
    pub vram_total_bytes: u64,

    /// List of currently detected blocking process names.
    pub blocking_processes: Vec<String>,

    /// Local IP address of this machine.
    pub local_ip: String,

    /// Model download/update status messages.
    pub model_status: Vec<String>,

    /// Whether model downloads are currently in progress.
    pub models_downloading: bool,
}

impl AppState {
    /// Creates a new AppState with default values derived from the given config.
    pub fn new(config: config::FreeCycleConfig) -> Self {
        let local_ip = local_ip_address::local_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        Self {
            status: state::FreeCycleStatus::Initializing,
            config,
            agent_task: None,
            manual_override: None,
            last_blacklist_seen: None,
            vram_idle_since: None,
            wake_block_until: None,
            ollama_running: false,
            vram_used_bytes: 0,
            vram_total_bytes: 0,
            blocking_processes: Vec::new(),
            local_ip,
            model_status: Vec::new(),
            models_downloading: false,
        }
    }
}

/// Shared async application state used across background subsystems.
pub type SharedAppState = Arc<RwLock<AppState>>;
