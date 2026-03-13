//! HTTP server for external agent task signaling and remote model installs.
//!
//! Listens on a configurable port (default 7443) for external agentic workflows
//! to signal task start/stop events. When a task starts, the tray icon turns blue
//! and the tooltip shows the task description.

use crate::logging::{scrub_http_preview, scrub_http_preview_default};
use crate::state::{AgentTask, FreeCycleStatus};
use crate::{ModelProgress, ModelTransferKind, SharedAppState};
use anyhow::Result;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{MatchedPath, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::Instant;
use tokio::sync::watch;
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

/// Request body for installing a model through FreeCycle.
#[derive(Debug, Deserialize)]
pub struct ModelInstallRequest {
    /// Full Ollama model name or tag to install.
    pub model_name: String,
}

/// Response body for status queries.
///
/// Returns the current FreeCycle status, VRAM info, and active task details.
#[derive(Debug, Deserialize, Serialize)]
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

    /// Whether the tray-controlled remote install window is currently open.
    pub remote_model_installs_unlocked: bool,

    /// Seconds until the remote install window closes, if currently open.
    pub remote_model_installs_expires_in_seconds: Option<u64>,
}

/// Generic success/error response.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiResponse {
    /// Whether the operation succeeded.
    pub ok: bool,

    /// Status message.
    pub message: String,
}

const TASK_DESCRIPTION_PREVIEW_CHARS: usize = 120;
type SharedState = SharedAppState;

enum ParsedJson<T> {
    Value(T),
    EarlyResponse(Response),
}

fn source_ip_from_request(request: &axum::http::Request<Body>) -> String {
    request
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|connect_info| connect_info.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn matched_path_or_uri(request: &axum::http::Request<Body>) -> String {
    request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched_path| matched_path.as_str().to_string())
        .unwrap_or_else(|| request.uri().path().to_string())
}

async fn log_http_request_response(request: axum::http::Request<Body>, next: Next) -> Response {
    let method = request.method().clone();
    let path = matched_path_or_uri(&request);
    let source_ip = source_ip_from_request(&request);

    debug!(
        "HTTP request: method={} path={} from={}",
        method, path, source_ip
    );

    let response = next.run(request).await;

    debug!(
        "HTTP response: method={} path={} from={} status={}",
        method,
        path,
        source_ip,
        response.status()
    );

    response
}

fn log_json_rejection(path: &str, source_ip: &str, rejection: &JsonRejection) {
    let detail = scrub_http_preview_default(&rejection.body_text());
    warn!(
        "HTTP request rejected: method=POST path={} from={} status={} detail='{}'",
        path,
        source_ip,
        rejection.status(),
        detail
    );
}

fn parse_json_or_reject<T>(
    result: Result<Json<T>, JsonRejection>,
    path: &str,
    source_ip: &str,
) -> ParsedJson<T> {
    match result {
        Ok(Json(value)) => ParsedJson::Value(value),
        Err(rejection) => {
            log_json_rejection(path, source_ip, &rejection);
            ParsedJson::EarlyResponse(rejection.into_response())
        }
    }
}

pub fn build_agent_server_router(state: SharedAppState) -> Router {
    Router::new()
        .route("/status", get(handle_status))
        .route("/task/start", post(handle_task_start))
        .route("/task/stop", post(handle_task_stop))
        .route("/models/install", post(handle_model_install))
        .route("/health", get(handle_health))
        .layer(middleware::from_fn(log_http_request_response))
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
/// * `POST /models/install` - Install a model when the tray unlock window is active.
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
    state: SharedAppState,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let (bind_address, port) = {
        let s = state.read().await;
        (
            s.config.agent_server.bind_address.clone(),
            s.config.agent_server.port,
        )
    };

    let app = build_agent_server_router(state);

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
    axum::extract::ConnectInfo(_addr): axum::extract::ConnectInfo<SocketAddr>,
) -> Json<StatusResponse> {
    let mut s = state.write().await;
    let now = Instant::now();
    if s.clear_expired_remote_model_install_unlock(now) {
        info!("Remote model install unlock expired");
    }
    let vram_percent = if s.vram_total_bytes > 0 {
        s.vram_used_bytes * 100 / s.vram_total_bytes
    } else {
        0
    };
    let remote_model_install_remaining = s
        .remote_model_install_unlock_remaining(now)
        .map(|duration| duration.as_secs().max(1));

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
        remote_model_installs_unlocked: remote_model_install_remaining.is_some(),
        remote_model_installs_expires_in_seconds: remote_model_install_remaining,
    })
}

/// Handles POST /task/start requests.
///
/// Registers an external agent task. The tray icon turns blue and the tooltip
/// shows the task description with the source IP.
async fn handle_task_start(
    State(state): State<SharedState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    payload: Result<Json<TaskStartRequest>, JsonRejection>,
) -> Response {
    let source_ip = addr.ip().to_string();
    let req = match parse_json_or_reject(payload, "/task/start", &source_ip) {
        ParsedJson::Value(req) => req,
        ParsedJson::EarlyResponse(response) => return response,
    };
    let task_id = scrub_http_preview_default(&req.task_id);
    let description_preview = scrub_http_preview(&req.description, TASK_DESCRIPTION_PREVIEW_CHARS);

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
        )
            .into_response();
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
        .into_response()
}

/// Handles POST /task/stop requests.
///
/// Clears the tracked agent task. The tray icon reverts to green (Available).
async fn handle_task_stop(
    State(state): State<SharedState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    payload: Result<Json<TaskStopRequest>, JsonRejection>,
) -> Response {
    let source_ip = addr.ip().to_string();
    let req = match parse_json_or_reject(payload, "/task/stop", &source_ip) {
        ParsedJson::Value(req) => req,
        ParsedJson::EarlyResponse(response) => return response,
    };
    let task_id = scrub_http_preview_default(&req.task_id);
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
            )
                .into_response();
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
        .into_response()
}

fn status_blocks_remote_install(status: &FreeCycleStatus) -> bool {
    matches!(
        status,
        FreeCycleStatus::Blocked
            | FreeCycleStatus::Cooldown { .. }
            | FreeCycleStatus::WakeDelay { .. }
    )
}

fn remote_install_locked_message() -> String {
    "Remote model installs are locked. Enable the tray menu toggle to allow installs for the next hour.".to_string()
}

fn remote_install_failure_message(model_name: &str, error: &anyhow::Error) -> String {
    let detail = scrub_http_preview(&error.to_string(), 160);
    if detail.is_empty() {
        format!("Failed: {} (remote install failed)", model_name)
    } else {
        format!("Failed: {} ({})", model_name, detail)
    }
}

async fn handle_model_install(
    State(state): State<SharedState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    payload: Result<Json<ModelInstallRequest>, JsonRejection>,
) -> Response {
    let source_ip = addr.ip().to_string();
    let req = match parse_json_or_reject(payload, "/models/install", &source_ip) {
        ParsedJson::Value(req) => req,
        ParsedJson::EarlyResponse(response) => return response,
    };
    let model_name = scrub_http_preview_default(&req.model_name);

    info!(
        "Remote model install received: model='{}', from={}",
        model_name, source_ip
    );

    let base_url = {
        let mut s = state.write().await;
        let now = Instant::now();
        if s.clear_expired_remote_model_install_unlock(now) {
            info!("Remote model install unlock expired");
        }

        if !s.remote_model_install_unlocked(now) {
            warn!(
                "Rejecting remote model install '{}': tray unlock is disabled",
                model_name
            );
            return (
                StatusCode::FORBIDDEN,
                Json(ApiResponse {
                    ok: false,
                    message: remote_install_locked_message(),
                }),
            )
                .into_response();
        }

        if status_blocks_remote_install(&s.status) {
            warn!(
                "Rejecting remote model install '{}': GPU is currently {}",
                model_name,
                s.status.label()
            );
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse {
                    ok: false,
                    message: format!("GPU is currently {}", s.status.label()),
                }),
            )
                .into_response();
        }

        if !s.ollama_running {
            warn!(
                "Rejecting remote model install '{}': Ollama is not running",
                model_name
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiResponse {
                    ok: false,
                    message: "Ollama is not running".to_string(),
                }),
            )
                .into_response();
        }

        s.upsert_model_progress(ModelProgress::new(
            req.model_name.clone(),
            ModelTransferKind::Downloading,
        ));

        format!("http://127.0.0.1:{}", s.config.ollama.port)
    };

    match crate::ollama::pull_model(
        std::sync::Arc::clone(&state),
        &base_url,
        &req.model_name,
        ModelTransferKind::Downloading,
    )
    .await
    {
        Ok(()) => {
            let mut s = state.write().await;
            s.remove_model_progress(&req.model_name);
            (
                StatusCode::OK,
                Json(ApiResponse {
                    ok: true,
                    message: format!("Model '{}' installed", req.model_name),
                }),
            )
                .into_response()
        }
        Err(error) => {
            let failure_status = remote_install_failure_message(&req.model_name, &error);
            warn!(
                "Remote model install failed for '{}': {}",
                model_name, failure_status
            );
            let mut s = state.write().await;
            s.upsert_model_progress(ModelProgress {
                model_name: req.model_name.clone(),
                kind: ModelTransferKind::Downloading,
                percent: None,
                status_text: failure_status,
                failed: true,
            });
            (
                StatusCode::BAD_GATEWAY,
                Json(ApiResponse {
                    ok: false,
                    message: format!("Model '{}' failed to install", req.model_name),
                }),
            )
                .into_response()
        }
    }
}

/// Handles GET /health requests.
///
/// Simple health check that returns 200 OK.
async fn handle_health(
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
) -> (StatusCode, Json<ApiResponse>) {
    let _ = addr;
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
    use crate::AppState;
    use reqwest::Client;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use tokio::sync::RwLock;

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
            remote_model_installs_unlocked: false,
            remote_model_installs_expires_in_seconds: None,
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
            remote_model_install_unlocked_until: None,
            ollama_running: false,
            vram_used_bytes: 0,
            vram_total_bytes: 0,
            blocking_processes: vec!["VRChat.exe".to_string()],
            local_ip: "192.168.1.10".to_string(),
            model_progress: Vec::new(),
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
                build_agent_server_router(state)
                    .into_make_service_with_connect_info::<SocketAddr>(),
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

    #[tokio::test]
    async fn test_status_reports_remote_model_install_unlock_state() {
        let state = test_state(FreeCycleStatus::Available);
        {
            let mut state_guard = state.write().await;
            state_guard.remote_model_install_unlocked_until =
                Some(Instant::now() + Duration::from_secs(90));
        }

        let (base_url, shutdown_tx, server) = spawn_test_server(Arc::clone(&state)).await;
        let client = Client::new();

        let response = client
            .get(format!("{}/status", base_url))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let status = response.json::<StatusResponse>().await.unwrap();
        assert!(status.remote_model_installs_unlocked);
        assert!(
            status
                .remote_model_installs_expires_in_seconds
                .expect("unlock seconds")
                >= 1
        );

        shutdown_test_server(shutdown_tx, server).await;
    }

    #[tokio::test]
    async fn test_model_install_returns_forbidden_when_unlock_is_disabled() {
        let state = test_state(FreeCycleStatus::Available);
        {
            let mut state_guard = state.write().await;
            state_guard.ollama_running = true;
        }

        let (base_url, shutdown_tx, server) = spawn_test_server(Arc::clone(&state)).await;
        let client = Client::new();

        let response = client
            .post(format!("{}/models/install", base_url))
            .json(&json!({
                "model_name": "qwen2.5-coder:7b",
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.json::<ApiResponse>().await.unwrap(),
            ApiResponse {
                ok: false,
                message: remote_install_locked_message(),
            }
        );

        shutdown_test_server(shutdown_tx, server).await;
    }
}
