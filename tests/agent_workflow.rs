use freecycle::agent_server::{build_agent_server_router, ApiResponse, StatusResponse};
use freecycle::config::FreeCycleConfig;
use freecycle::state::FreeCycleStatus;
use freecycle::AppState;
use reqwest::Client;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::sync::RwLock;

type SharedState = Arc<RwLock<AppState>>;

fn test_state() -> SharedState {
    Arc::new(RwLock::new(AppState::new(FreeCycleConfig::default())))
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
            build_agent_server_router(state).into_make_service_with_connect_info::<SocketAddr>(),
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
async fn simulated_agent_workflow_start_status_stop() {
    let state = test_state();
    let (base_url, shutdown_tx, server) = spawn_test_server(Arc::clone(&state)).await;
    let client = Client::new();

    let start_response = client
        .post(format!("{}/task/start", base_url))
        .json(&json!({
            "task_id": "workflow-1",
            "description": "Integration workflow",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(start_response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        start_response.json::<ApiResponse>().await.unwrap(),
        ApiResponse {
            ok: true,
            message: "Task 'workflow-1' registered".to_string(),
        }
    );

    let status_response = client
        .get(format!("{}/status", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(status_response.status(), reqwest::StatusCode::OK);
    let status = status_response.json::<StatusResponse>().await.unwrap();
    assert_eq!(status.status, FreeCycleStatus::AgentTaskActive.label());
    assert_eq!(status.active_task_id.as_deref(), Some("workflow-1"));
    assert_eq!(
        status.active_task_description.as_deref(),
        Some("Integration workflow")
    );
    assert_eq!(status.ollama_port, 11434);

    {
        let state_guard = state.read().await;
        assert_eq!(state_guard.status, FreeCycleStatus::AgentTaskActive);
        assert_eq!(status.local_ip, state_guard.local_ip);
        assert_eq!(
            state_guard
                .agent_task
                .as_ref()
                .map(|task| task.source_ip.as_str()),
            Some("127.0.0.1")
        );
    }

    let stop_response = client
        .post(format!("{}/task/stop", base_url))
        .json(&json!({
            "task_id": "workflow-1",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(stop_response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        stop_response.json::<ApiResponse>().await.unwrap(),
        ApiResponse {
            ok: true,
            message: "Task 'workflow-1' stopped".to_string(),
        }
    );

    let final_status_response = client
        .get(format!("{}/status", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(final_status_response.status(), reqwest::StatusCode::OK);
    let final_status = final_status_response
        .json::<StatusResponse>()
        .await
        .unwrap();
    assert_eq!(final_status.status, FreeCycleStatus::Available.label());
    assert_eq!(final_status.active_task_id, None);
    assert_eq!(final_status.active_task_description, None);

    {
        let state_guard = state.read().await;
        assert_eq!(state_guard.status, FreeCycleStatus::Available);
        assert!(state_guard.agent_task.is_none());
    }

    shutdown_test_server(shutdown_tx, server).await;
}

#[tokio::test]
async fn simulated_blocked_state_transition_rejects_then_recovers() {
    let state = test_state();
    {
        let mut state_guard = state.write().await;
        state_guard.status = FreeCycleStatus::Blocked;
        state_guard.blocking_processes = vec!["VRChat.exe".to_string()];
    }

    let (base_url, shutdown_tx, server) = spawn_test_server(Arc::clone(&state)).await;
    let client = Client::new();

    let blocked_start_response = client
        .post(format!("{}/task/start", base_url))
        .json(&json!({
            "task_id": "blocked-workflow",
            "description": "Should be rejected while blocked",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(
        blocked_start_response.status(),
        reqwest::StatusCode::CONFLICT
    );
    assert_eq!(
        blocked_start_response.json::<ApiResponse>().await.unwrap(),
        ApiResponse {
            ok: false,
            message: "GPU is currently Blocked (Game Running)".to_string(),
        }
    );

    let blocked_status_response = client
        .get(format!("{}/status", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(blocked_status_response.status(), reqwest::StatusCode::OK);
    let blocked_status = blocked_status_response
        .json::<StatusResponse>()
        .await
        .unwrap();
    assert_eq!(blocked_status.status, FreeCycleStatus::Blocked.label());
    assert_eq!(blocked_status.active_task_id, None);
    assert_eq!(blocked_status.active_task_description, None);
    assert_eq!(blocked_status.blocking_processes, vec!["VRChat.exe"]);

    {
        let state_guard = state.read().await;
        assert_eq!(state_guard.status, FreeCycleStatus::Blocked);
        assert!(state_guard.agent_task.is_none());
        assert_eq!(state_guard.blocking_processes, vec!["VRChat.exe"]);
    }

    {
        let mut state_guard = state.write().await;
        state_guard.status = FreeCycleStatus::Available;
        state_guard.blocking_processes.clear();
    }

    let recovered_start_response = client
        .post(format!("{}/task/start", base_url))
        .json(&json!({
            "task_id": "recovered-workflow",
            "description": "Accepted after block clears",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(recovered_start_response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        recovered_start_response
            .json::<ApiResponse>()
            .await
            .unwrap(),
        ApiResponse {
            ok: true,
            message: "Task 'recovered-workflow' registered".to_string(),
        }
    );

    let recovered_status_response = client
        .get(format!("{}/status", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(recovered_status_response.status(), reqwest::StatusCode::OK);
    let recovered_status = recovered_status_response
        .json::<StatusResponse>()
        .await
        .unwrap();
    assert_eq!(
        recovered_status.status,
        FreeCycleStatus::AgentTaskActive.label()
    );
    assert_eq!(
        recovered_status.active_task_id.as_deref(),
        Some("recovered-workflow")
    );
    assert_eq!(
        recovered_status.active_task_description.as_deref(),
        Some("Accepted after block clears")
    );
    assert!(recovered_status.blocking_processes.is_empty());

    {
        let state_guard = state.read().await;
        assert_eq!(state_guard.status, FreeCycleStatus::AgentTaskActive);
        let task = state_guard
            .agent_task
            .as_ref()
            .expect("agent task recorded");
        assert_eq!(task.task_id, "recovered-workflow");
        assert_eq!(task.description, "Accepted after block clears");
        assert_eq!(task.source_ip, "127.0.0.1");
        assert!(task.started_at <= Instant::now());
        assert!(state_guard.blocking_processes.is_empty());
    }

    shutdown_test_server(shutdown_tx, server).await;
}

#[tokio::test]
async fn malformed_json_request_returns_bad_request_without_mutating_state() {
    let state = test_state();
    let (base_url, shutdown_tx, server) = spawn_test_server(Arc::clone(&state)).await;
    let client = Client::new();

    let response = client
        .post(format!("{}/task/start", base_url))
        .header("Content-Type", "application/json")
        .body(r#"{"task_id":"broken","description":"missing quote}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = response.text().await.unwrap();
    assert!(body.contains("Failed to parse the request body as JSON"));

    {
        let state_guard = state.read().await;
        assert_eq!(state_guard.status, FreeCycleStatus::Initializing);
        assert!(state_guard.agent_task.is_none());
    }

    shutdown_test_server(shutdown_tx, server).await;
}

#[tokio::test]
async fn status_reports_remote_model_install_unlock_window() {
    let state = test_state();
    {
        let mut state_guard = state.write().await;
        state_guard.status = FreeCycleStatus::Available;
        state_guard.remote_model_install_unlocked_until =
            Some(Instant::now() + Duration::from_secs(120));
    }

    let (base_url, shutdown_tx, server) = spawn_test_server(Arc::clone(&state)).await;
    let client = Client::new();

    let response = client
        .get(format!("{}/status", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);
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
async fn model_install_requires_tray_unlock() {
    let state = test_state();
    {
        let mut state_guard = state.write().await;
        state_guard.status = FreeCycleStatus::Available;
        state_guard.ollama_running = true;
    }

    let (base_url, shutdown_tx, server) = spawn_test_server(Arc::clone(&state)).await;
    let client = Client::new();

    let response = client
        .post(format!("{}/models/install", base_url))
        .json(&json!({
            "model_name": "llama3.2:3b",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    assert_eq!(
        response.json::<ApiResponse>().await.unwrap(),
        ApiResponse {
            ok: false,
            message: "Remote model installs are locked. Enable the tray menu toggle to allow installs for the next hour.".to_string(),
        }
    );

    shutdown_test_server(shutdown_tx, server).await;
}
