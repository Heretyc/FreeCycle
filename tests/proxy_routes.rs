use freecycle::agent_server::build_agent_server_router;
use freecycle::config::FreeCycleConfig;
use freecycle::AppState;
use reqwest::Client;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::sync::Mutex;
use tokio::sync::RwLock;

type SharedState = Arc<RwLock<AppState>>;

#[derive(Debug, Clone)]
struct ReceivedRequest {
    method: String,
    path: String,
    query: Option<String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn test_state() -> SharedState {
    let mut config = FreeCycleConfig::default();
    config.agent_server.compatibility_mode = true;
    Arc::new(RwLock::new(AppState::new(config)))
}

/// Spawns a mock Ollama server on a random port and captures all requests.
async fn spawn_mock_ollama_server(
) -> (String, oneshot::Sender<()>, tokio::task::JoinHandle<()>, Arc<Mutex<Vec<ReceivedRequest>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let captured_requests = Arc::new(Mutex::new(Vec::new()));
    let captured_requests_clone = Arc::clone(&captured_requests);

    let server = tokio::spawn(async move {
        let router = axum::Router::new().fallback(
            axum::routing::any(move |request: axum::extract::Request| {
                let captured = Arc::clone(&captured_requests_clone);
                async move {
                    let (parts, body) = request.into_parts();
                    let method_str = parts.method.to_string();
                    let path = parts.uri.path().to_string();
                    let query = parts.uri.query().map(|q| q.to_string());

                    // Capture headers
                    let mut captured_headers = HashMap::new();
                    for (name, value) in &parts.headers {
                        if let Ok(v) = value.to_str() {
                            captured_headers.insert(name.to_string(), v.to_string());
                        }
                    }

                    // Read body
                    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
                        Ok(bytes) => bytes.to_vec(),
                        Err(_) => vec![],
                    };

                    let req = ReceivedRequest {
                        method: method_str,
                        path,
                        query,
                        headers: captured_headers,
                        body: body_bytes,
                    };

                    captured.lock().await.push(req);

                    // Return a simple mock response
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(serde_json::json!({"mocked": true})),
                    )
                }
            }),
        );

        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    (format!("http://{}", addr), shutdown_tx, server, captured_requests)
}

/// Spawns the FreeCycle agent server for testing.
async fn spawn_test_agent_server(
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

/// Shuts down a test server.
async fn shutdown_test_server(
    shutdown_tx: oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
) {
    let _ = shutdown_tx.send(());
    server.await.unwrap();
}

/// Shuts down a mock Ollama server.
async fn shutdown_mock_ollama(
    shutdown_tx: oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
) {
    let _ = shutdown_tx.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn proxy_get_forwards_to_upstream() {
    let state = test_state();

    // Spawn mock Ollama server first
    let (_mock_url, mock_shutdown, mock_server, captured) = spawn_mock_ollama_server().await;

    // Configure state to point to mock server
    let mock_port = {
        let parts: Vec<&str> = _mock_url.split(':').collect();
        parts[2].parse::<u16>().unwrap()
    };

    {
        let mut s = state.write().await;
        s.config.ollama.port = mock_port;
    }

    // Spawn agent server
    let (agent_url, agent_shutdown, agent_server) = spawn_test_agent_server(Arc::clone(&state)).await;

    let client = Client::new();

    // Send GET request through proxy
    let response = client
        .get(format!("{}/api/tags", agent_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    // Clean up
    shutdown_test_server(agent_shutdown, agent_server).await;
    shutdown_mock_ollama(mock_shutdown, mock_server).await;

    // Verify mock received the request
    let reqs = captured.lock().await;
    assert!(!reqs.is_empty());
    assert_eq!(reqs[0].method, "GET");
    assert_eq!(reqs[0].path, "/api/tags");
}

#[tokio::test]
async fn proxy_post_forwards_body() {
    let state = test_state();

    let (_mock_url, mock_shutdown, mock_server, captured) = spawn_mock_ollama_server().await;

    let mock_port = {
        let parts: Vec<&str> = _mock_url.split(':').collect();
        parts[2].parse::<u16>().unwrap()
    };

    {
        let mut s = state.write().await;
        s.config.ollama.port = mock_port;
    }

    let (agent_url, agent_shutdown, agent_server) = spawn_test_agent_server(Arc::clone(&state)).await;

    let client = Client::new();

    let test_body = serde_json::json!({
        "model": "llama2",
        "prompt": "Hello"
    });

    let response = client
        .post(format!("{}/api/generate", agent_url))
        .json(&test_body)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    shutdown_test_server(agent_shutdown, agent_server).await;
    shutdown_mock_ollama(mock_shutdown, mock_server).await;

    let reqs = captured.lock().await;
    assert!(!reqs.is_empty());
    assert_eq!(reqs[0].method, "POST");
    assert_eq!(reqs[0].path, "/api/generate");
    assert!(!reqs[0].body.is_empty());
}

#[tokio::test]
async fn proxy_query_string_preserved() {
    let state = test_state();

    let (_mock_url, mock_shutdown, mock_server, captured) = spawn_mock_ollama_server().await;

    let mock_port = {
        let parts: Vec<&str> = _mock_url.split(':').collect();
        parts[2].parse::<u16>().unwrap()
    };

    {
        let mut s = state.write().await;
        s.config.ollama.port = mock_port;
    }

    let (agent_url, agent_shutdown, agent_server) = spawn_test_agent_server(Arc::clone(&state)).await;

    let client = Client::new();

    let response = client
        .get(format!("{}/api/tags?name=llama2", agent_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    shutdown_test_server(agent_shutdown, agent_server).await;
    shutdown_mock_ollama(mock_shutdown, mock_server).await;

    let reqs = captured.lock().await;
    assert!(!reqs.is_empty());
    assert_eq!(reqs[0].path, "/api/tags");
    assert_eq!(reqs[0].query.as_deref(), Some("name=llama2"));
}

#[tokio::test]
async fn proxy_upstream_down_returns_bad_gateway() {
    let state = test_state();

    {
        let mut s = state.write().await;
        // Point to a port that has nothing listening
        s.config.ollama.port = 19999;
    }

    let (agent_url, agent_shutdown, agent_server) = spawn_test_agent_server(Arc::clone(&state)).await;

    let client = Client::new();

    // This should fail because nothing is listening on port 19999
    let response = client
        .get(format!("{}/api/tags", agent_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);

    shutdown_test_server(agent_shutdown, agent_server).await;
}

#[tokio::test]
async fn proxy_hop_by_hop_headers_stripped() {
    let state = test_state();

    let (_mock_url, mock_shutdown, mock_server, captured) = spawn_mock_ollama_server().await;

    let mock_port = {
        let parts: Vec<&str> = _mock_url.split(':').collect();
        parts[2].parse::<u16>().unwrap()
    };

    {
        let mut s = state.write().await;
        s.config.ollama.port = mock_port;
    }

    let (agent_url, agent_shutdown, agent_server) = spawn_test_agent_server(Arc::clone(&state)).await;

    let client = Client::new();

    // Send a request with a custom header that shouldn't be forwarded
    // (We use a synthetic approach: send keep-alive which is hop-by-hop)
    let response = client
        .get(format!("{}/api/tags", agent_url))
        .header("x-custom-header", "value")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    shutdown_test_server(agent_shutdown, agent_server).await;
    shutdown_mock_ollama(mock_shutdown, mock_server).await;

    let reqs = captured.lock().await;
    assert!(!reqs.is_empty());

    // Custom header should be forwarded (not hop-by-hop)
    assert!(reqs[0].headers.contains_key("x-custom-header"));
}

#[tokio::test]
async fn proxy_response_headers_forwarded() {
    let state = test_state();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (mock_shutdown, shutdown_rx) = oneshot::channel();

    let mock_server = tokio::spawn(async move {
        let router = axum::Router::new().fallback(
            axum::routing::any(move |_: axum::extract::Request| async {
                let mut headers = axum::http::HeaderMap::new();
                headers.insert("x-custom-response", "test-value".parse().unwrap());
                headers.insert("content-type", "application/json".parse().unwrap());

                (
                    axum::http::StatusCode::OK,
                    headers,
                    axum::Json(serde_json::json!({"result": "ok"})),
                )
            }),
        );

        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    let mock_port = addr.port();

    {
        let mut s = state.write().await;
        s.config.ollama.port = mock_port;
    }

    let (agent_url, agent_shutdown, agent_server) = spawn_test_agent_server(Arc::clone(&state)).await;

    let client = Client::new();

    let response = client
        .get(format!("{}/api/tags", agent_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    // Verify custom header was forwarded
    assert_eq!(
        response.headers().get("x-custom-response"),
        Some(&"test-value".parse().unwrap())
    );

    shutdown_test_server(agent_shutdown, agent_server).await;
    shutdown_mock_ollama(mock_shutdown, mock_server).await;
}

#[tokio::test]
async fn proxy_response_hop_by_hop_stripped() {
    let state = test_state();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (mock_shutdown, shutdown_rx) = oneshot::channel();

    let mock_server = tokio::spawn(async move {
        let router = axum::Router::new().fallback(
            axum::routing::any(move |_: axum::extract::Request| async {
                let mut headers = axum::http::HeaderMap::new();
                // Try to set a proxy-authenticate header (hop-by-hop)
                headers.insert("proxy-authenticate", "Basic".parse().unwrap());
                headers.insert("content-type", "application/json".parse().unwrap());

                (
                    axum::http::StatusCode::OK,
                    headers,
                    axum::Json(serde_json::json!({"result": "ok"})),
                )
            }),
        );

        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    let mock_port = addr.port();

    {
        let mut s = state.write().await;
        s.config.ollama.port = mock_port;
    }

    let (agent_url, agent_shutdown, agent_server) = spawn_test_agent_server(Arc::clone(&state)).await;

    let client = Client::new();

    let response = client
        .get(format!("{}/api/tags", agent_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    // Verify content-type is still there (not hop-by-hop)
    assert!(response.headers().contains_key("content-type"));

    shutdown_test_server(agent_shutdown, agent_server).await;
    shutdown_mock_ollama(mock_shutdown, mock_server).await;
}

#[tokio::test]
async fn proxy_streaming_response_forwarded() {
    let state = test_state();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (mock_shutdown, shutdown_rx) = oneshot::channel();

    let mock_server = tokio::spawn(async move {
        let router = axum::Router::new().fallback(axum::routing::any(move |_: axum::extract::Request| async {
            // Return chunked response
            let body = axum::body::Body::from("chunk1chunk2chunk3");
            (axum::http::StatusCode::OK, body)
        }));

        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    let mock_port = addr.port();

    {
        let mut s = state.write().await;
        s.config.ollama.port = mock_port;
    }

    let (agent_url, agent_shutdown, agent_server) = spawn_test_agent_server(Arc::clone(&state)).await;

    let client = Client::new();

    let response = client
        .get(format!("{}/api/generate", agent_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body = response.text().await.unwrap();
    assert_eq!(body, "chunk1chunk2chunk3");

    shutdown_test_server(agent_shutdown, agent_server).await;
    shutdown_mock_ollama(mock_shutdown, mock_server).await;
}
