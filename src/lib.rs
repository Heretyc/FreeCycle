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
pub mod notifications;
pub mod ollama;
pub mod state;
pub mod tray;

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

pub const REMOTE_MODEL_INSTALL_UNLOCK_DURATION: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTransferKind {
    Downloading,
    Updating,
}

impl ModelTransferKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Downloading => "Downloading",
            Self::Updating => "Updating",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelProgress {
    pub model_name: String,
    pub kind: ModelTransferKind,
    pub percent: Option<u8>,
    pub status_text: String,
    pub failed: bool,
}

impl ModelProgress {
    pub fn new(model_name: impl Into<String>, kind: ModelTransferKind) -> Self {
        Self {
            model_name: model_name.into(),
            kind,
            percent: None,
            status_text: kind.label().to_string(),
            failed: false,
        }
    }

    pub fn render_status(&self) -> String {
        if self.failed {
            return self.status_text.clone();
        }

        match self.percent {
            Some(percent) => format!("{} {}: {}%", self.kind.label(), self.model_name, percent),
            None if self.status_text.eq_ignore_ascii_case(self.kind.label()) => {
                format!("{} {}", self.kind.label(), self.model_name)
            }
            None => format!(
                "{} {}: {}",
                self.kind.label(),
                self.model_name,
                self.status_text
            ),
        }
    }
}

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

    /// Deadline until which remote agents may request ad hoc model installs.
    pub remote_model_install_unlocked_until: Option<Instant>,

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

    /// Structured in-memory model transfer state for tooltip progress updates.
    pub model_progress: Vec<ModelProgress>,

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
            remote_model_install_unlocked_until: None,
            ollama_running: false,
            vram_used_bytes: 0,
            vram_total_bytes: 0,
            blocking_processes: Vec::new(),
            local_ip,
            model_progress: Vec::new(),
            model_status: Vec::new(),
            models_downloading: false,
        }
    }

    fn sync_model_status(&mut self) {
        self.models_downloading = self.model_progress.iter().any(|progress| !progress.failed);
        self.model_status = self
            .model_progress
            .iter()
            .map(ModelProgress::render_status)
            .collect();
    }

    pub fn upsert_model_progress(&mut self, progress: ModelProgress) {
        if let Some(existing) = self
            .model_progress
            .iter_mut()
            .find(|existing| existing.model_name == progress.model_name)
        {
            *existing = progress;
        } else {
            self.model_progress.push(progress);
        }

        self.sync_model_status();
    }

    pub fn remove_model_progress(&mut self, model_name: &str) {
        self.model_progress
            .retain(|progress| progress.model_name != model_name);
        self.sync_model_status();
    }

    pub fn remote_model_install_unlock_remaining(&self, now: Instant) -> Option<Duration> {
        self.remote_model_install_unlocked_until
            .and_then(|deadline| deadline.checked_duration_since(now))
    }

    pub fn remote_model_install_unlocked(&self, now: Instant) -> bool {
        self.remote_model_install_unlock_remaining(now).is_some()
    }

    pub fn clear_expired_remote_model_install_unlock(&mut self, now: Instant) -> bool {
        if self.remote_model_install_unlock_remaining(now).is_none()
            && self.remote_model_install_unlocked_until.is_some()
        {
            self.remote_model_install_unlocked_until = None;
            return true;
        }

        false
    }

    pub fn enable_remote_model_install_unlock(&mut self, now: Instant) -> Instant {
        let deadline = now + REMOTE_MODEL_INSTALL_UNLOCK_DURATION;
        self.remote_model_install_unlocked_until = Some(deadline);
        deadline
    }

    pub fn disable_remote_model_install_unlock(&mut self) {
        self.remote_model_install_unlocked_until = None;
    }
}

/// Shared async application state used across background subsystems.
pub type SharedAppState = Arc<RwLock<AppState>>;
