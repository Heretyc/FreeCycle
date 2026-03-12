//! HTTP server for external agent task signaling.
//!
//! Listens on a configurable port (default 7443) for external agentic workflows
//! to signal task start/stop events. When a task starts, the tray icon turns blue
//! and the tooltip shows the task description.

use crate::logging::{scrub_http_preview, scrub_http_preview_default};
use crate::state::{AgentTask, FreeCycleStatus};
use crate::AppState;
use anyhow::Result;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{watch, RwLock};
use tracing::{debug, info, warn};

/// Request body for starting a task.
///
/// Sent by external agents to signal they are beginning GPU work.
///
/// # Fields
///
/// * `task_id` - Unique identifier for the task.
/// * `description` - Human-readable description of what the agent is doing.
#[derive(Debug, Deserialize)]
pub struct TaskStartRequest {
    /// Unique identifier for the task.
    pub task_id: String,

    /// Human-readable description of the GPU work being performed.
    pub description: String,
}

/// Request body for stopping a task.
///
/// Sent by external agents to signal they have finished GPU work.
///
/// # Fields
///
/// * `task_id` - The task identifier that was provided in the start signal.
#[derive(Debug, Deserialize)]
pub struct TaskStopRequest {
    /// The task identifier to stop tracking.
    pub task_id: String,
}

/// Response body for status queries.
///
/// Returns the current FreeCycle status, VRAM info, and active task details.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    /// Current status label (e.g., "Available", "Blocked").
    pub status: String,

    /// Whether Ollama is currently running.
    pub ollama_running: bool,

    /// VRAM used in megabytes.
    pub vram_used_mb: u64,

    /// Total VRAM in megabytes.
    pub vram_total_mb: u64,

    /// VRAM usage as a percentage.
    pub vram_percent: u64,

    /// Currently active agent task ID, if any.
    pub active_task_id: Option<String>,

    /// Currently active agent task description, if any.
    pub active_task_description: Option<String>,

    /// Local IP address.
    pub local_ip: String,

    /// Ollama port.
    pub ollama_port: u16,

    /// List of blocking process names.
    pub blocking_processes: Vec<String>,

    /// Model download status messages.
    pub model_status: Vec<String>,
}

/// Generic success/error response.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiResponse {
    /// Whether the operation succeeded.
    pub ok: bool,

    /// Status message.
    pub message: String,
}

/// Shared state type alias for axum extractors.
type SharedState = Arc<RwLock<AppState>>;

const TASK_DESCRIPTION_PREVIEW_CHARS: usize = 120;

fn log_request(method: &str, path: &str, source_ip: &str) {
    debug!("HTTP request: {} {} from {}", method, path, source_ip);
}

fn build_router(state: SharedState) -> Router {
    Router::new()
        .route("/status", get(handle_status))
        .route("/task/start", post(handle_task_start))
        .route("/task/stop", post(handle_task_stop))
        .route("/health", get(handle_health))
        .with_state(state)
}

/// Runs the agent signal HTTP server.
///
/// Binds to the configured address and port (default `0.0.0.0:7443`) and
/// exposes endpoints for task signaling and status queries.
///
/// # Endpoints
///
/// * `GET /status` - Returns current FreeCycle status.
/// * `POST /task/start` - Signal the start of an external GPU task.
/// * `POST /task/stop` - Signal the end of an external GPU task.
/// * `GET /health` - Simple health check endpoint.
///
/// # Arguments
///
/// * `state` - Shared application state.
/// * `shutdown_rx` - Watch channel for shutdown signal.
///
/// # Errors
///
/// Returns an error if the server fails to bind or encounters a fatal error.
pub async fn run_agent_server(
    state: SharedState,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let (bind_address, port) = {
        let s = state.read().await;
        (
            s.config.agent_server.bind_address.clone(),
            s.config.agent_server.port,
        )
    };

    let app = build_router(state);

    let addr: SocketAddr = format!("{}:{}", bind_address, port)
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid bind address: {}", e))?;

    info!("Agent signal server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    let make_service = app.into_make_service_with_connect_info::<SocketAddr>();

    axum::serve(listener, make_service)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.changed().await;
        })
        .await?;

    Ok(())
}

/// Handles GET /status requests.
///
/// Returns the current application status, VRAM usage, active task info,
/// and other relevant state.
async fn handle_status(
    State(state): State<SharedState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
) -> Json<StatusResponse> {
    log_request("GET", "/status", &addr.ip().to_string());
    let s = state.read().await;
    let vram_percent = if s.vram_total_bytes > 0 {
        s.vram_used_bytes * 100 / s.vram_total_bytes
    } else {
        0
    };

    Json(StatusResponse {
        status: s.status.label().to_string(),
        ollama_running: s.ollama_running,
        vram_used_mb: s.vram_used_bytes / (1024 * 1024),
        vram_total_mb: s.vram_total_bytes / (1024 * 1024),
        vram_percent,
        active_task_id: s.agent_task.as_ref().map(|t| t.task_id.clone()),
        active_task_description: s.agent_task.as_ref().map(|t| t.description.clone()),
        local_ip: s.local_ip.clone(),
        ollama_port: s.config.ollama.port,
        blocking_processes: s.blocking_processes.clone(),
        model_status: s.model_status.clone(),
    })
}

/// Handles POST /task/start requests.
///
/// Registers an external agent task. The tray icon turns blue and the tooltip
/// shows the task description with the source IP.
async fn handle_task_start(
    State(state): State<SharedState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    Json(req): Json<TaskStartRequest>,
) -> (StatusCode, Json<ApiResponse>) {
    let source_ip = addr.ip().to_string();
    let task_id = scrub_http_preview_default(&req.task_id);
    let description_preview = scrub_http_preview(&req.description, TASK_DESCRIPTION_PREVIEW_CHARS);
    log_request("POST", "/task/start", &source_ip);

    info!(
        "Agent task start received: id='{}', from={}",
        task_id, source_ip
    );
    debug!(
        "Agent task start description preview: '{}'",
        description_preview
    );

    let mut s = state.write().await;

    // Only accept task signals when GPU is available (not blocked/cooldown)
    if matches!(
        s.status,
        FreeCycleStatus::Blocked
            | FreeCycleStatus::Cooldown { .. }
            | FreeCycleStatus::WakeDelay { .. }
    ) {
        warn!(
            "Rejecting task start '{}': GPU is currently {}",
            task_id,
            s.status.label()
        );
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse {
                ok: false,
                message: format!("GPU is currently {}", s.status.label()),
            }),
        );
    }

    s.agent_task = Some(AgentTask {
        task_id: req.task_id.clone(),
        description: req.description.clone(),
        started_at: Instant::now(),
        source_ip,
    });
    s.status = FreeCycleStatus::AgentTaskActive;
    s.vram_idle_since = None;

    (
        StatusCode::OK,
        Json(ApiResponse {
            ok: true,
            message: format!("Task '{}' registered", req.task_id),
        }),
    )
}

/// Handles POST /task/stop requests.
///
/// Clears the tracked agent task. The tray icon reverts to green (Available).
async fn handle_task_stop(
    State(state): State<SharedState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    Json(req): Json<TaskStopRequest>,
) -> (StatusCode, Json<ApiResponse>) {
    let source_ip = addr.ip().to_string();
    let task_id = scrub_http_preview_default(&req.task_id);
    log_request("POST", "/task/stop", &source_ip);
    info!(
        "Agent task stop received: id='{}', from={}",
        task_id, source_ip
    );

    let mut s = state.write().await;

    if let Some(ref task) = s.agent_task {
        if task.task_id == req.task_id {
            s.agent_task = None;
            s.vram_idle_since = None;
            if matches!(s.status, FreeCycleStatus::AgentTaskActive) {
                s.status = FreeCycleStatus::Available;
            }
            return (
                StatusCode::OK,
                Json(ApiResponse {
                    ok: true,
                    message: format!("Task '{}' stopped", req.task_id),
                }),
            );
        }
    }

    warn!(
        "Agent task stop ignored: id='{}', from={}, reason=task_not_found",
        task_id, source_ip
    );

    (
        StatusCode::NOT_FOUND,
        Json(ApiResponse {
            ok: false,
            message: format!("Task '{}' not found", req.task_id),
        }),
    )
}

/// Handles GET /health requests.
///
/// Simple health check that returns 200 OK.
async fn handle_health(
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
) -> (StatusCode, Json<ApiResponse>) {
    log_request("GET", "/health", &addr.ip().to_string());
    (
        StatusCode::OK,
        Json(ApiResponse {
            ok: true,
            message: "FreeCycle is running".to_string(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FreeCycleConfig;
    use crate::state::FreeCycleStatus;
    use reqwest::Client;
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    #[test]
    fn test_task_start_request_deserialization() {
        let json = r#"{"task_id": "test-1", "description": "Running batch inference"}"#;
        let req: TaskStartRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.task_id, "test-1");
        assert_eq!(req.description, "Running batch inference");
    }

    #[test]
    fn test_task_stop_request_deserialization() {
        let json = r#"{"task_id": "test-1"}"#;
        let req: TaskStopRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.task_id, "test-1");
    }

    #[test]
    fn test_status_response_serialization() {
        let resp = StatusResponse {
            status: "Available".to_string(),
            ollama_running: true,
            vram_used_mb: 1024,
            vram_total_mb: 8192,
            vram_percent: 12,
            active_task_id: None,
            active_task_description: None,
            local_ip: "192.168.1.10".to_string(),
            ollama_port: 11434,
            blocking_processes: vec![],
            model_status: vec![],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("Available"));
    }

    fn test_state(status: FreeCycleStatus) -> SharedState {
        Arc::new(RwLock::new(AppState {
            status,
            config: FreeCycleConfig::default(),
            agent_task: None,
            manual_override: None,
            last_blacklist_seen: None,
            vram_idle_since: None,
            wake_block_until: None,
            ollama_running: false,
            vram_used_bytes: 0,
            vram_total_bytes: 0,
            blocking_processes: vec!["VRChat.exe".to_string()],
            local_ip: "192.168.1.10".to_string(),
            model_status: Vec::new(),
            models_downloading: false,
        }))
    }

    async fn spawn_test_server(
        state: SharedState,
    ) -> (String, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                build_router(state).into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
        });

        (format!("http://{}", addr), shutdown_tx, server)
    }

    async fn shutdown_test_server(
        shutdown_tx: oneshot::Sender<()>,
        server: tokio::task::JoinHandle<()>,
    ) {
        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_task_start_records_remote_ip_via_connect_info() {
        let state = test_state(FreeCycleStatus::Available);
        let (base_url, shutdown_tx, server) = spawn_test_server(Arc::clone(&state)).await;
        let client = Client::new();

        let response = client
            .post(format!("{}/task/start", base_url))
            .json(&json!({
                "task_id": "task-1",
                "description": "Testing ConnectInfo",
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.json::<ApiResponse>().await.unwrap(),
            ApiResponse {
                ok: true,
                message: "Task 'task-1' registered".to_string(),
            }
        );

        let state_guard = state.read().await;
        let task = state_guard.agent_task.as_ref().unwrap();
        assert_eq!(task.task_id, "task-1");
        assert_eq!(task.description, "Testing ConnectInfo");
        assert_eq!(task.source_ip, "127.0.0.1");
        assert_eq!(state_guard.status, FreeCycleStatus::AgentTaskActive);
        drop(state_guard);

        shutdown_test_server(shutdown_tx, server).await;
    }

    #[tokio::test]
    async fn test_task_start_returns_conflict_when_gpu_is_blocked() {
        let state = test_state(FreeCycleStatus::Blocked);
        let (base_url, shutdown_tx, server) = spawn_test_server(Arc::clone(&state)).await;
        let client = Client::new();

        let response = client
            .post(format!("{}/task/start", base_url))
            .json(&json!({
                "task_id": "blocked-task",
                "description": "Should not start",
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.json::<ApiResponse>().await.unwrap(),
            ApiResponse {
                ok: false,
                message: "GPU is currently Blocked (Game Running)".to_string(),
            }
        );

        let state_guard = state.read().await;
        assert!(state_guard.agent_task.is_none());
        assert_eq!(state_guard.status, FreeCycleStatus::Blocked);
        drop(state_guard);

        shutdown_test_server(shutdown_tx, server).await;
    }

    #[tokio::test]
    async fn test_task_stop_requires_matching_id_before_clearing_state() {
        let state = test_state(FreeCycleStatus::AgentTaskActive);
        {
            let mut state_guard = state.write().await;
            state_guard.agent_task = Some(AgentTask {
                task_id: "task-1".to_string(),
                description: "Tracked task".to_string(),
                started_at: Instant::now(),
                source_ip: "127.0.0.1".to_string(),
            });
        }

        let (base_url, shutdown_tx, server) = spawn_test_server(Arc::clone(&state)).await;
        let client = Client::new();

        let mismatch = client
            .post(format!("{}/task/stop", base_url))
            .json(&json!({
                "task_id": "other-task",
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(mismatch.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            mismatch.json::<ApiResponse>().await.unwrap(),
            ApiResponse {
                ok: false,
                message: "Task 'other-task' not found".to_string(),
            }
        );

        {
            let state_guard = state.read().await;
            assert_eq!(
                state_guard
                    .agent_task
                    .as_ref()
                    .map(|task| task.task_id.as_str()),
                Some("task-1")
            );
            assert_eq!(state_guard.status, FreeCycleStatus::AgentTaskActive);
        }

        let matched = client
            .post(format!("{}/task/stop", base_url))
            .json(&json!({
                "task_id": "task-1",
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(matched.status(), StatusCode::OK);
        assert_eq!(
            matched.json::<ApiResponse>().await.unwrap(),
            ApiResponse {
                ok: true,
                message: "Task 'task-1' stopped".to_string(),
            }
        );

        let state_guard = state.read().await;
        assert!(state_guard.agent_task.is_none());
        assert_eq!(state_guard.status, FreeCycleStatus::Available);
        drop(state_guard);

        shutdown_test_server(shutdown_tx, server).await;
    }
}
