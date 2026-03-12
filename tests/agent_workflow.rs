use freecycle::agent_server::{build_agent_server_router, ApiResponse, StatusResponse};
use freecycle::config::FreeCycleConfig;
use freecycle::state::FreeCycleStatus;
use freecycle::AppState;
use reqwest::Client;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
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
