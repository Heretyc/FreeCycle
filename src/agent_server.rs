//! HTTP server for external agent task signaling and remote model installs.
//!
//! Listens on a configurable port (default 7443) for external agentic workflows
//! to signal task start/stop events. When a task starts, the tray icon turns blue
//! and the tooltip shows the task description.

use crate::logging::{scrub_http_preview, scrub_http_preview_default};
use crate::model_catalog;
use crate::state::{AgentTask, FreeCycleStatus};
use crate::{ModelProgress, ModelTransferKind, SharedAppState};
use anyhow::Result;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{MatchedPath, State};
use axum::http::{HeaderName, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
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
/// When secure mode is enabled, also includes identity fields (server UUID, Ed25519 pubkey,
/// TLS cert fingerprint, and GPU fingerprint).
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

    /// Server UUID (present in secure mode, None in compatibility mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_uuid: Option<String>,

    /// Ed25519 public key in base64 encoding (present in secure mode, None in compatibility mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ed25519_pubkey: Option<String>,

    /// TLS certificate SHA-256 fingerprint (present in secure mode, None in compatibility mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_cert_fingerprint: Option<String>,

    /// GPU fingerprint string (present in secure mode, None in compatibility mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_fingerprint: Option<String>,
}

/// Response body for identity queries.
///
/// Returns server identity information including UUID, Ed25519 public key,
/// TLS certificate fingerprint, and GPU fingerprint.
#[derive(Debug, Serialize, Deserialize)]
pub struct IdentityResponse {
    /// Server UUID (hyphenated string).
    pub server_uuid: String,

    /// Ed25519 public key in base64 encoding (32 raw bytes).
    pub ed25519_pubkey: Option<String>,

    /// TLS certificate SHA-256 fingerprint in lowercase hex (64 chars), or None in compatibility mode.
    pub tls_cert_fingerprint: Option<String>,

    /// GPU fingerprint: "{local_ip} with {gpu_name} @ {vram_total_mb}MB VRAM"
    pub gpu_fingerprint: String,
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

fn is_hop_by_hop_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str().to_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "transfer-encoding"
            | "te"
            | "trailer"
            | "proxy-authorization"
            | "proxy-authenticate"
            | "upgrade"
    )
}

pub fn build_agent_server_router(state: SharedAppState) -> Router {
    Router::new()
        .route("/status", get(handle_status))
        .route("/identity", get(handle_identity))
        .route("/task/start", post(handle_task_start))
        .route("/task/stop", post(handle_task_stop))
        .route("/models/install", post(handle_model_install))
        .route("/models/catalog", get(handle_catalog))
        .route("/health", get(handle_health))
        // Catch-all route for Ollama API proxy must be last
        .route("/api/*path", any(handle_ollama_proxy))
        .layer(middleware::from_fn(log_http_request_response))
        .with_state(state)
}

/// Handles Ollama API proxy requests.
///
/// Proxies HTTP requests to Ollama running on 127.0.0.1, preserving method,
/// request body, and non-hop-by-hop headers. Streams large responses to avoid
/// buffering in memory.
async fn handle_ollama_proxy(
    State(state): State<SharedState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
) -> Response {
    let source_ip = addr.ip().to_string();
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(|q| q.to_string());

    debug!(
        "Ollama proxy request: method={} path={} from={}",
        method, path, source_ip
    );

    // Read ollama address from state
    let (ollama_host, ollama_port) = {
        let s = state.read().await;
        (s.config.ollama.secure_host.clone(), s.config.ollama.port)
    };

    // Construct target URL
    let target_url = if let Some(q) = query {
        format!("http://{}:{}{path}?{q}", ollama_host, ollama_port)
    } else {
        format!("http://{}:{}{path}", ollama_host, ollama_port)
    };

    // Read request body
    let (parts, body) = request.into_parts();
    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => {
            warn!(
                "Ollama proxy: failed to read request body for path={} from={}",
                path, source_ip
            );
            return (
                StatusCode::BAD_REQUEST,
                "Failed to read request body",
            )
                .into_response();
        }
    };

    // Build reqwest client with 1-hour timeout
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3600))
        .build()
        .unwrap_or_default();

    // Build the request to Ollama
    let mut upstream_request = match parts.method {
        axum::http::Method::GET => client.get(&target_url),
        axum::http::Method::POST => client.post(&target_url),
        axum::http::Method::PUT => client.put(&target_url),
        axum::http::Method::DELETE => client.delete(&target_url),
        axum::http::Method::PATCH => client.patch(&target_url),
        axum::http::Method::HEAD => client.head(&target_url),
        axum::http::Method::OPTIONS => client.request(axum::http::Method::OPTIONS, &target_url),
        method => {
            warn!(
                "Ollama proxy: unsupported HTTP method {} for path={} from={}",
                method, path, source_ip
            );
            return (
                StatusCode::BAD_REQUEST,
                "Unsupported HTTP method",
            )
                .into_response();
        }
    };

    // Forward non-hop-by-hop headers from the original request
    for (name, value) in &parts.headers {
        if !is_hop_by_hop_header(name) {
            if let Ok(v) = std::str::from_utf8(value.as_bytes()) {
                upstream_request = upstream_request.header(name.clone(), v);
            }
        }
    }

    // Add request body if not empty
    if !body_bytes.is_empty() {
        upstream_request = upstream_request.body(body_bytes);
    }

    // Send the request to Ollama
    let upstream_response = match upstream_request.send().await {
        Ok(resp) => resp,
        Err(err) => {
            warn!(
                "Ollama proxy: upstream connection failed for path={} from={}: {}",
                path, source_ip, err
            );
            return (
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect to Ollama: {}", err),
            )
                .into_response();
        }
    };

    debug!(
        "Ollama proxy response: method={} path={} from={} status={}",
        method,
        path,
        source_ip,
        upstream_response.status()
    );

    // Build response headers, filtering out hop-by-hop headers
    let mut response_headers = axum::http::HeaderMap::new();
    for (name, value) in upstream_response.headers() {
        if !is_hop_by_hop_header(name) {
            response_headers.insert(name.clone(), value.clone());
        }
    }

    let status = upstream_response.status();

    // Stream the response body chunk-by-chunk; necessary for /api/generate, /api/chat, /api/pull.
    let response_body = axum::body::Body::from_stream(upstream_response.bytes_stream());

    (status, response_headers, response_body).into_response()
}

/// Runs the agent signal HTTP server.
///
/// Binds to the configured address and port (default `0.0.0.0:7443`) and
/// exposes endpoints for task signaling and status queries.
///
/// When `config.agent_server.compatibility_mode` is false (default):
/// - Serves TLS using axum-server with rustls
/// - Reads certificate and key from security config paths
/// - Returns an error if certificate files are missing
///
/// When `config.agent_server.compatibility_mode` is true:
/// - Serves plaintext HTTP using axum (legacy behavior)
///
/// # Endpoints
///
/// * `GET /status` - Returns current FreeCycle status.
/// * `GET /identity` - Returns server identity (UUID, pubkey, TLS fingerprint, GPU fingerprint).
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
/// Returns an error if the server fails to bind, encounters a fatal error,
/// or (in secure mode) if TLS certificate files are missing or malformed.
/// Returns true if the error is "address already in use".
fn is_addr_in_use(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::AddrInUse {
        return true;
    }
    // Windows WSAEADDRINUSE (10048) may not map to AddrInUse on all toolchains
    #[cfg(windows)]
    if e.raw_os_error() == Some(10048) {
        return true;
    }
    false
}

pub async fn run_agent_server(
    state: SharedAppState,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let (bind_address, port, compatibility_mode) = {
        let s = state.read().await;
        (
            s.config.agent_server.bind_address.clone(),
            s.config.agent_server.port,
            s.config.agent_server.compatibility_mode,
        )
    };

    let app = build_agent_server_router(state.clone());

    let addr: SocketAddr = format!("{}:{}", bind_address, port)
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid bind address: {}", e))?;

    // Retry parameters for EADDRINUSE — the old process may still be
    // releasing the port after being killed during stale-lock takeover.
    const MAX_BIND_RETRIES: u32 = 4;
    const BIND_RETRY_DELAY: Duration = Duration::from_secs(2);

    if compatibility_mode {
        // Plaintext HTTP mode (legacy behavior)
        let listener = {
            let mut listener = None;
            for attempt in 0..=MAX_BIND_RETRIES {
                match tokio::net::TcpListener::bind(addr).await {
                    Ok(l) => {
                        listener = Some(l);
                        break;
                    }
                    Err(ref e) if is_addr_in_use(e) && attempt < MAX_BIND_RETRIES => {
                        warn!(
                            "Port {} in use (attempt {}/{}), retrying in {}s",
                            port,
                            attempt + 1,
                            MAX_BIND_RETRIES,
                            BIND_RETRY_DELAY.as_secs()
                        );
                        tokio::time::sleep(BIND_RETRY_DELAY).await;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            listener.ok_or_else(|| anyhow::anyhow!("Port {} still in use after retries", port))?
        };

        info!("Agent signal server listening on {} (plaintext)", addr);
        let make_service = app.into_make_service_with_connect_info::<SocketAddr>();

        axum::serve(listener, make_service)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.changed().await;
            })
            .await?;
    } else {
        // TLS mode (secure mode, default)

        // Load TLS certificate and key
        let (cert_path, key_path) = {
            let s = state.read().await;
            crate::security::tls_cert_and_key_paths(&s.config.security)
        };

        let tls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to load TLS certificate from {:?} and key from {:?}: {}",
                    cert_path,
                    key_path,
                    e
                )
            })?;

        // Retry bind loop for EADDRINUSE
        let mut last_err: Option<std::io::Error> = None;
        for attempt in 0..=MAX_BIND_RETRIES {
            // Pre-check with a plain TCP bind so we can detect EADDRINUSE
            // before committing to the full TLS server setup.
            match tokio::net::TcpListener::bind(addr).await {
                Ok(probe) => {
                    // Port is free — drop probe listener before axum-server binds
                    drop(probe);
                    last_err = None;
                    break;
                }
                Err(ref e) if is_addr_in_use(e) && attempt < MAX_BIND_RETRIES => {
                    warn!(
                        "Port {} in use (attempt {}/{}), retrying in {}s",
                        port,
                        attempt + 1,
                        MAX_BIND_RETRIES,
                        BIND_RETRY_DELAY.as_secs()
                    );
                    tokio::time::sleep(BIND_RETRY_DELAY).await;
                    last_err = Some(std::io::Error::new(e.kind(), e.to_string()));
                }
                Err(e) => return Err(e.into()),
            }
        }
        if let Some(e) = last_err {
            return Err(anyhow::anyhow!("Port {} still in use after retries: {}", port, e));
        }

        info!("Agent signal server listening on {} (TLS)", addr);

        let handle = axum_server::Handle::new();
        let handle_clone = handle.clone();
        tokio::spawn(async move {
            let _ = shutdown_rx.changed().await;
            handle_clone.graceful_shutdown(Some(Duration::from_secs(5)));
        });

        axum_server::bind_rustls(addr, tls_config)
            .handle(handle)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;
    }

    Ok(())
}

/// Handles GET /status requests.
///
/// Returns the current application status, VRAM usage, active task info,
/// and other relevant state. In secure mode (compatibility_mode=false),
/// also includes identity fields.
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

    // Determine if we're in secure mode (compatibility_mode=false means secure mode is enabled)
    let compatibility_mode = s.config.agent_server.compatibility_mode;

    // Prepare identity fields based on mode
    let (server_uuid, ed25519_pubkey, tls_cert_fingerprint, gpu_fingerprint) =
        if compatibility_mode {
            // In compatibility mode, all identity fields are None
            (None, None, None, None)
        } else {
            // In secure mode, clone security config and call helpers to get identity fields
            let security_config = s.config.security.clone();
            let server_uuid = security_config
                .identity_uuid
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let ed25519_pubkey = crate::security::read_verifying_key_base64(&security_config);
            let tls_cert_fingerprint = crate::security::read_tls_cert_fingerprint(&security_config);
            let gpu_fingerprint = crate::security::build_gpu_fingerprint(&security_config);

            (
                Some(server_uuid),
                ed25519_pubkey,
                tls_cert_fingerprint,
                Some(gpu_fingerprint),
            )
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
        remote_model_installs_unlocked: remote_model_install_remaining.is_some(),
        remote_model_installs_expires_in_seconds: remote_model_install_remaining,
        server_uuid,
        ed25519_pubkey,
        tls_cert_fingerprint,
        gpu_fingerprint,
    })
}

/// Handles GET /identity requests.
///
/// Returns server identity information including UUID, Ed25519 public key,
/// TLS certificate fingerprint, and GPU fingerprint.
async fn handle_identity(
    State(state): State<SharedState>,
    axum::extract::ConnectInfo(_addr): axum::extract::ConnectInfo<SocketAddr>,
) -> Json<IdentityResponse> {
    let s = state.read().await;

    // Read the server UUID (should always exist after startup)
    let server_uuid = s
        .config
        .security
        .identity_uuid
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    // Read Ed25519 public key (base64-encoded)
    let ed25519_pubkey = crate::security::read_verifying_key_base64(&s.config.security);

    // Read TLS certificate fingerprint (SHA-256 hex)
    let tls_cert_fingerprint = crate::security::read_tls_cert_fingerprint(&s.config.security);

    // Build GPU fingerprint
    let gpu_fingerprint = crate::security::build_gpu_fingerprint(&s.config.security);

    Json(IdentityResponse {
        server_uuid,
        ed25519_pubkey,
        tls_cert_fingerprint,
        gpu_fingerprint,
    })
}

/// Validates a task description according to Priority 10 constraints.
///
/// Rules enforced:
/// 1. Length must be 30-40 Unicode scalar values (characters).
/// 2. No single character may dominate more than 60% of the description.
/// 3. Description must contain at least one alphanumeric character (not all whitespace/punctuation).
/// 4. No single word (2+ chars) may appear in more than 60% of qualifying words (min 3 words required).
///
/// Returns `Ok(())` if the description is valid, or `Err(&'static str)` with a brief error message.
fn validate_task_description(description: &str) -> Result<(), &'static str> {
    // Check 1: Length must be 30-40 characters
    let len = description.chars().count();
    if !(30..=40).contains(&len) {
        return Err("Task description must be 30-40 characters");
    }

    // Check 2: No char dominance (>60% of description)
    let mut char_freq = std::collections::HashMap::new();
    let total_chars = description.chars().count();
    for ch in description.chars() {
        let lower = ch.to_ascii_lowercase();
        *char_freq.entry(lower).or_insert(0) += 1;
    }
    if let Some(&max_freq) = char_freq.values().max() {
        if max_freq as f64 / total_chars as f64 > 0.60 {
            return Err("Task description appears to contain padding");
        }
    }

    // Check 3: Must contain at least one alphanumeric character
    if !description.chars().any(|c| c.is_alphanumeric()) {
        return Err("Task description appears to contain padding");
    }

    // Check 4: Repeated words dominance (>60% of qualifying words)
    let words: Vec<String> = description
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 2)
        .collect();

    if words.len() >= 3 {
        let mut word_freq = std::collections::HashMap::new();
        for word in &words {
            *word_freq.entry(word.clone()).or_insert(0) += 1;
        }
        if let Some(&max_freq) = word_freq.values().max() {
            if max_freq as f64 / words.len() as f64 > 0.60 {
                return Err("Task description appears to contain padding");
            }
        }
    }

    Ok(())
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

    // Validate task description before proceeding
    if let Err(msg) = validate_task_description(&req.description) {
        return (StatusCode::BAD_REQUEST, Json(ApiResponse {
            ok: false,
            message: msg.to_string(),
        }))
            .into_response();
    }

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

        format!("http://{}:{}", s.config.ollama.secure_host, s.config.ollama.port)
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

/// Handles GET /models/catalog requests.
///
/// Returns the model catalog JSON if available, or 404 if the catalog has not been generated yet.
async fn handle_catalog(
    axum::extract::ConnectInfo(_addr): axum::extract::ConnectInfo<SocketAddr>,
) -> Result<Json<model_catalog::ModelCatalog>, (StatusCode, Json<ApiResponse>)> {
    match model_catalog::load_catalog() {
        Ok(Some(catalog)) => Ok(Json(catalog)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ApiResponse {
                ok: false,
                message: "Model catalog not yet available".to_string(),
            }),
        )),
        Err(e) => {
            warn!("Failed to load model catalog: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    ok: false,
                    message: "Failed to load model catalog".to_string(),
                }),
            ))
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
            server_uuid: None,
            ed25519_pubkey: None,
            tls_cert_fingerprint: None,
            gpu_fingerprint: None,
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
            notification_hwnd: None,
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
                "description": "MCP generate: llama3.1:8b test",
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
        assert_eq!(task.description, "MCP generate: llama3.1:8b test");
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
                "description": "MCP generate: llama3.1:8b blocked",
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

    #[tokio::test]
    async fn test_identity_endpoint_returns_200_with_gpu_fingerprint() {
        let state = test_state(FreeCycleStatus::Available);
        let (base_url, shutdown_tx, server) = spawn_test_server(Arc::clone(&state)).await;
        let client = Client::new();

        let response = client
            .get(format!("{}/identity", base_url))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let identity = response.json::<IdentityResponse>().await.unwrap();

        // Verify server_uuid is populated
        assert!(!identity.server_uuid.is_empty());
        // (In test env, UUID won't be set, so it should be "unknown" or the default)

        // Verify gpu_fingerprint is non-empty
        assert!(!identity.gpu_fingerprint.is_empty());
        // gpu_fingerprint should follow the pattern "... with ... @ ...MB VRAM"
        assert!(identity.gpu_fingerprint.contains(" with "));
        assert!(identity.gpu_fingerprint.contains("MB VRAM"));

        // In test env, cert and pubkey files don't exist, so these should be None
        assert!(
            identity.ed25519_pubkey.is_none(),
            "Pubkey should be None in test env (no files)"
        );
        assert!(
            identity.tls_cert_fingerprint.is_none(),
            "Cert fingerprint should be None in test env (no files)"
        );

        shutdown_test_server(shutdown_tx, server).await;
    }

    #[tokio::test]
    async fn test_status_includes_identity_fields_in_secure_mode() {
        let state = test_state(FreeCycleStatus::Available);
        {
            let mut state_guard = state.write().await;
            // Set compatibility_mode to false (secure mode enabled)
            state_guard.config.agent_server.compatibility_mode = false;
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

        // Verify identity fields are present (Some) in secure mode
        assert!(status.server_uuid.is_some());
        // Note: ed25519_pubkey and tls_cert_fingerprint will be None in test env because files don't exist
        assert!(status.gpu_fingerprint.is_some());
        // Verify gpu_fingerprint follows expected pattern
        assert!(
            status
                .gpu_fingerprint
                .as_ref()
                .unwrap()
                .contains(" with ")
        );
        assert!(
            status
                .gpu_fingerprint
                .as_ref()
                .unwrap()
                .contains("MB VRAM")
        );

        shutdown_test_server(shutdown_tx, server).await;
    }

    #[tokio::test]
    async fn test_status_omits_identity_fields_in_compatibility_mode() {
        let state = test_state(FreeCycleStatus::Available);
        {
            let mut state_guard = state.write().await;
            // Set compatibility_mode to true (compatibility mode enabled)
            state_guard.config.agent_server.compatibility_mode = true;
        }

        let (base_url, shutdown_tx, server) = spawn_test_server(Arc::clone(&state)).await;
        let client = Client::new();

        let response = client
            .get(format!("{}/status", base_url))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.text().await.unwrap();

        // Verify the four identity fields are absent from the JSON response
        // (skip_serializing_if = "Option::is_none" ensures they are omitted)
        assert!(!body.contains("server_uuid"));
        assert!(!body.contains("ed25519_pubkey"));
        assert!(!body.contains("tls_cert_fingerprint"));
        assert!(!body.contains("gpu_fingerprint"));

        shutdown_test_server(shutdown_tx, server).await;
    }

    #[test]
    fn test_validate_task_description_length_boundaries() {
        // 29 chars: too short
        assert_eq!(
            validate_task_description("12345678901234567890123456789"),
            Err("Task description must be 30-40 characters")
        );
        // 30 chars: valid
        assert_eq!(
            validate_task_description("123456789012345678901234567890"),
            Ok(())
        );
        // 40 chars: valid
        assert_eq!(
            validate_task_description("1234567890123456789012345678901234567890"),
            Ok(())
        );
        // 41 chars: too long
        assert_eq!(
            validate_task_description("12345678901234567890123456789012345678901"),
            Err("Task description must be 30-40 characters")
        );
    }

    #[test]
    fn test_validate_task_description_char_dominance() {
        // 60%+ of one char (padding): "aaa...aaa test." (24/30 = 80%) - exactly 30 chars
        assert_eq!(
            validate_task_description("aaaaaaaaaaaaaaaaaaaaaaaa test "),
            Err("Task description appears to contain padding")
        );
        // Just under 60% is OK: "aaaa... test task ok." = 30 chars
        assert_eq!(
            validate_task_description("aaaaaaaaaaaaaaaa test task ok "),
            Ok(())
        );
    }

    #[test]
    fn test_validate_task_description_all_whitespace_punctuation() {
        // All punctuation/whitespace: no alphanumeric (30 chars exactly)
        assert_eq!(
            validate_task_description("!@#$%^&*() - - - - - - - -    "),
            Err("Task description appears to contain padding")
        );
        // Contains at least one letter: OK (30 chars exactly)
        assert_eq!(
            validate_task_description("!@#$%^&*() - - - - - a - -    "),
            Ok(())
        );
    }

    #[test]
    fn test_validate_task_description_repeated_words() {
        // Single word repeated 3+ times (>60%): 4 "test" out of 5 = 80%
        assert_eq!(
            validate_task_description("test test test test whatever x"),
            Err("Task description appears to contain padding")
        );
        // Word appears exactly 60% - we reject >60%, not >=60%
        // With 5 words, 3 = 60%, which should pass
        assert_eq!(
            validate_task_description("test test test word other here "),
            Ok(())
        );
    }

    #[test]
    fn test_validate_task_description_good_examples() {
        // Valid task descriptions (30-40 chars each)
        assert_eq!(
            validate_task_description("MCP generate: llama3.1:8b img "),
            Ok(())
        );
        assert_eq!(
            validate_task_description("MCP embed: nomic-embed-text f "),
            Ok(())
        );
        assert_eq!(
            validate_task_description("Running local chat inference m"),
            Ok(())
        );
    }
}
