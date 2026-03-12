//! Application state machine for FreeCycle.
//!
//! Defines the possible states the application can be in and the transitions
//! between them. The state drives tray icon appearance, Ollama lifecycle,
//! and tooltip content.

use std::time::Instant;

/// Represents the current operational status of FreeCycle.
///
/// The status determines tray icon color, Ollama process state,
/// and tooltip messaging. Transitions are driven by the GPU monitor
/// and agent signal server.
///
/// # State Diagram
///
/// ```text
/// Initializing -> Available (green) -> Blocked (red) -> Cooldown (red)
///                     ^                                       |
///                     +---------------------------------------+
///                     |
///                     v
///              AgentTaskActive (blue)
///
/// Any state -> Downloading (yellow) [overlay, not exclusive]
/// Any state -> Error (grey)
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum FreeCycleStatus {
    /// Application is starting up, performing initial checks.
    Initializing,

    /// GPU is available. Ollama is running and exposed to the network.
    Available,

    /// A blacklisted process (game) is currently running. Ollama is stopped.
    Blocked,

    /// A blacklisted process was recently detected. Cooldown period active.
    /// Ollama remains stopped until cooldown expires (1800 seconds default).
    Cooldown {
        /// When the cooldown period will expire.
        expires_at: Instant,
    },

    /// An external agent has signaled it is actively using the GPU for a task.
    /// Ollama is running. Icon is blue.
    AgentTaskActive,

    /// Models are being downloaded or updated. This is an overlay state
    /// that can coexist with Available or AgentTaskActive.
    Downloading,

    /// An error occurred (e.g., Ollama not installed, NVML init failed).
    Error(String),
}

impl FreeCycleStatus {
    /// Returns a human-readable label for the current status.
    ///
    /// # Returns
    ///
    /// A static string describing the status for tooltip display.
    pub fn label(&self) -> &str {
        match self {
            Self::Initializing => "Initializing",
            Self::Available => "Available",
            Self::Blocked => "Blocked (Game Running)",
            Self::Cooldown { .. } => "Cooldown",
            Self::AgentTaskActive => "Agent Task Active",
            Self::Downloading => "Downloading Models",
            Self::Error(_) => "Error",
        }
    }
}

/// Information about a task reported by an external agent workflow.
///
/// External agents signal task start/stop via the agent HTTP server.
/// While a task is active, the tray icon turns blue and the tooltip
/// shows the task description.
///
/// # Fields
///
/// * `task_id` - Unique identifier for the task (provided by the agent).
/// * `description` - Human-readable description of what the agent is doing.
/// * `started_at` - When the task started.
/// * `source_ip` - IP address of the agent that sent the signal.
#[derive(Debug, Clone)]
pub struct AgentTask {
    /// Unique identifier for the task (provided by the agent).
    pub task_id: String,

    /// Human-readable description of what the agent is doing.
    pub description: String,

    /// When the task was started.
    pub started_at: Instant,

    /// IP address of the agent that sent the signal.
    pub source_ip: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_labels() {
        assert_eq!(FreeCycleStatus::Initializing.label(), "Initializing");
        assert_eq!(FreeCycleStatus::Available.label(), "Available");
        assert_eq!(FreeCycleStatus::Blocked.label(), "Blocked (Game Running)");
        assert_eq!(
            FreeCycleStatus::Cooldown {
                expires_at: Instant::now()
            }
            .label(),
            "Cooldown"
        );
        assert_eq!(FreeCycleStatus::AgentTaskActive.label(), "Agent Task Active");
        assert_eq!(FreeCycleStatus::Downloading.label(), "Downloading Models");
        assert_eq!(
            FreeCycleStatus::Error("test".into()).label(),
            "Error"
        );
    }

    #[test]
    fn test_agent_task_creation() {
        let task = AgentTask {
            task_id: "task-001".to_string(),
            description: "Running inference batch".to_string(),
            started_at: Instant::now(),
            source_ip: "192.168.1.50".to_string(),
        };
        assert_eq!(task.task_id, "task-001");
        assert_eq!(task.description, "Running inference batch");
    }
}
