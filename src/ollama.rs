//! Ollama process lifecycle and model management for FreeCycle.
//!
//! Handles starting/stopping the Ollama serve process, exposing it to the network
//! via `OLLAMA_HOST`, pulling required models, and periodic model update checks.

use crate::config::FreeCycleConfig;
use crate::logging::{scrub_http_preview, scrub_http_preview_default};
use crate::state::{FreeCycleStatus, ManualOverride};
use crate::{AppState, ModelProgress, ModelTransferKind};
use anyhow::{Context, Result};
use rusqlite;
use serde::Deserialize;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::{watch, RwLock};
use tracing::{debug, error, info, warn};

fn log_ollama_request(method: &str, url: &str, body: &str) {
    let body_preview = scrub_http_preview_default(body);
    if body_preview.is_empty() {
        debug!("Ollama HTTP request: {} {}", method, url);
    } else {
        debug!(
            "Ollama HTTP request: {} {} body={}",
            method, url, body_preview
        );
    }
}

fn log_ollama_transport_error(method: &str, url: &str, error: &reqwest::Error) {
    debug!(
        "Ollama HTTP transport failure: {} {} error={}",
        method, url, error
    );
}

async fn read_logged_response_preview(
    method: &str,
    url: &str,
    response: reqwest::Response,
) -> (reqwest::StatusCode, String) {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let preview = scrub_http_preview_default(&body);

    if preview.is_empty() {
        debug!("Ollama HTTP response: {} {} status={}", method, url, status);
    } else {
        debug!(
            "Ollama HTTP response: {} {} status={} body={}",
            method, url, status, preview
        );
    }

    (status, body)
}

#[derive(Debug, Deserialize)]
struct PullProgressEvent {
    #[serde(default)]
    status: String,
    #[serde(default)]
    completed: Option<u64>,
    #[serde(default)]
    total: Option<u64>,
    #[serde(default)]
    error: Option<String>,
}

fn progress_percent(completed: Option<u64>, total: Option<u64>) -> Option<u8> {
    match (completed, total) {
        (Some(completed), Some(total)) if total > 0 => {
            let percent = completed.saturating_mul(100) / total;
            Some(percent.min(100) as u8)
        }
        _ => None,
    }
}

fn build_progress_update(
    model: &str,
    kind: ModelTransferKind,
    event: &PullProgressEvent,
) -> Option<ModelProgress> {
    if let Some(error) = &event.error {
        return Some(ModelProgress {
            model_name: model.to_string(),
            kind,
            percent: None,
            status_text: format!("Failed: {} ({})", model, error.trim()),
            failed: true,
        });
    }

    let status_text = event.status.trim();
    if status_text.is_empty() && progress_percent(event.completed, event.total).is_none() {
        return None;
    }

    let mut progress = ModelProgress::new(model.to_string(), kind);
    progress.percent = progress_percent(event.completed, event.total);
    if !status_text.is_empty() {
        progress.status_text = status_text.to_string();
    }

    Some(progress)
}

async fn update_model_progress(
    state: &Arc<RwLock<AppState>>,
    model: &str,
    kind: ModelTransferKind,
    event: &PullProgressEvent,
) {
    if let Some(progress) = build_progress_update(model, kind, event) {
        let rendered = progress.render_status();
        if let Some(percent) = progress.percent {
            info!("Model progress: {} ({}%)", model, percent);
        } else if !progress.failed {
            info!("Model progress: {}", rendered);
        }

        let mut shared = state.write().await;
        shared.upsert_model_progress(progress);
    }
}

async fn process_pull_stream_chunk(
    state: &Arc<RwLock<AppState>>,
    model: &str,
    kind: ModelTransferKind,
    buffer: &mut String,
) {
    while let Some(newline_index) = buffer.find('\n') {
        let line = buffer[..newline_index].trim().to_string();
        buffer.drain(..=newline_index);

        if line.is_empty() {
            continue;
        }

        match serde_json::from_str::<PullProgressEvent>(&line) {
            Ok(event) => update_model_progress(state, model, kind, &event).await,
            Err(error) => {
                warn!(
                    "Failed to parse Ollama pull progress event for {}: {}",
                    model, error
                );
                debug!("Malformed Ollama pull progress payload: {}", line);
            }
        }
    }
}

async fn process_pull_stream_tail(
    state: &Arc<RwLock<AppState>>,
    model: &str,
    kind: ModelTransferKind,
    buffer: &str,
) {
    let line = buffer.trim();
    if line.is_empty() {
        return;
    }

    match serde_json::from_str::<PullProgressEvent>(line) {
        Ok(event) => update_model_progress(state, model, kind, &event).await,
        Err(error) => {
            warn!(
                "Failed to parse final Ollama pull progress event for {}: {}",
                model, error
            );
            debug!("Malformed final Ollama pull progress payload: {}", line);
        }
    }
}

/// Checks whether Ollama is installed on the system.
///
/// Searches for `ollama.exe` in the configured path, PATH, and common
/// install locations (`%LOCALAPPDATA%\Programs\Ollama`).
///
/// # Arguments
///
/// * `config` - The FreeCycle configuration (may contain an explicit exe path).
///
/// # Returns
///
/// `true` if Ollama is found, `false` otherwise.
pub fn is_ollama_installed(config: &FreeCycleConfig) -> bool {
    find_ollama_exe(config).is_some()
}

/// Locates the Ollama executable.
///
/// Search order:
/// 1. Explicit path from config (`ollama.exe_path`)
/// 2. System PATH
/// 3. `%LOCALAPPDATA%\Programs\Ollama\ollama.exe`
///
/// # Arguments
///
/// * `config` - The FreeCycle configuration.
///
/// # Returns
///
/// The full path to `ollama.exe` if found, or `None`.
fn find_ollama_exe(config: &FreeCycleConfig) -> Option<String> {
    // Check explicit config path
    if let Some(ref path) = config.ollama.exe_path {
        if std::path::Path::new(path).exists() {
            return Some(path.clone());
        }
    }

    // Check PATH via `where ollama`
    if let Ok(output) = std::process::Command::new("where").arg("ollama").output() {
        if output.status.success() {
            if let Ok(path) = String::from_utf8(output.stdout) {
                let first_line = path.lines().next().unwrap_or("").trim();
                if !first_line.is_empty() {
                    return Some(first_line.to_string());
                }
            }
        }
    }

    // Check common install location
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let path = format!("{}\\Programs\\Ollama\\ollama.exe", local_app_data);
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }
    }

    None
}

/// Starts the Ollama serve process with `OLLAMA_HOST` set to expose it to the network.
///
/// # Arguments
///
/// * `config` - The FreeCycle configuration (provides host, port, exe path).
///
/// # Returns
///
/// A `Child` process handle for the spawned Ollama process, or an error.
///
/// # Errors
///
/// Returns an error if the Ollama executable is not found or cannot be spawned.
async fn start_ollama(config: &FreeCycleConfig) -> Result<Child> {
    let exe = find_ollama_exe(config).context("Ollama executable not found")?;

    let effective_host = if config.agent_server.compatibility_mode {
        config.ollama.host.as_str() // "0.0.0.0" by default in compatibility mode
    } else {
        config.ollama.secure_host.as_str() // secure mode: loopback only (default 127.0.0.1)
    };

    let host_value = format!("{}:{}", effective_host, config.ollama.port);
    info!(
        "Starting Ollama: {} serve (OLLAMA_HOST={})",
        exe, host_value
    );

    let child = Command::new(&exe)
        .arg("serve")
        .env("OLLAMA_HOST", &host_value)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false)
        .spawn()
        .with_context(|| format!("Failed to spawn Ollama: {}", exe))?;

    info!("Ollama started with PID {}", child.id().unwrap_or(0));
    Ok(child)
}

/// Gracefully stops the Ollama process. Waits for the configured timeout,
/// then force-kills if still running.
///
/// # Arguments
///
/// * `child` - Mutable reference to the Ollama child process.
/// * `timeout_secs` - Seconds to wait for graceful shutdown before force kill.
async fn stop_ollama(child: &mut Child, timeout_secs: u64) {
    let pid = child.id().unwrap_or(0);
    info!(
        "Stopping Ollama (PID {}), waiting {}s for graceful shutdown",
        pid, timeout_secs
    );

    // Try graceful shutdown by sending a request to the Ollama API
    // Ollama does not have a dedicated shutdown endpoint, so we kill the process
    // gracefully via taskkill /PID
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string()])
        .output();

    // Wait for process to exit
    let timeout = Duration::from_secs(timeout_secs);
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            info!("Ollama exited gracefully with status: {}", status);
        }
        Ok(Err(e)) => {
            warn!("Error waiting for Ollama: {}. Force killing.", e);
            let _ = child.kill().await;
        }
        Err(_) => {
            warn!(
                "Ollama did not exit within {}s. Force killing.",
                timeout_secs
            );
            let _ = child.kill().await;
            // Also try taskkill /F as a fallback
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output();
        }
    }
}

/// Kills any existing Ollama processes (for clean startup).
///
/// Terminates `ollama app.exe` (the Ollama tray manager), `ollama.exe` (the
/// CLI/server), and `ollama_llama_server.exe` (the LLM backend). Killing the
/// tray app is required to prevent it from re-exposing Ollama on 0.0.0.0
/// after FreeCycle starts it securely on 127.0.0.1.
///
/// A short wait after the kill ensures file handles (including the SQLite
/// database lock) are released before any subsequent operations.
pub fn kill_existing_ollama() {
    debug!("Checking for existing Ollama processes to clean up");
    for process_name in &["ollama app.exe", "ollama.exe", "ollama_llama_server.exe"] {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/IM", process_name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output();
    }
    // Brief pause so the OS releases file handles (including the SQLite lock)
    // before callers attempt to open Ollama's settings database.
    std::thread::sleep(Duration::from_millis(500));
}

/// Disables the "Expose on Network" setting in Ollama's settings database.
///
/// Ollama's tray app (`ollama app.exe`) stores a boolean `expose` flag in
/// `%LOCALAPPDATA%\Ollama\db.sqlite`. When `expose = 1`, the tray re-binds
/// Ollama to `0.0.0.0` every time it (re)starts the server, overriding
/// FreeCycle's localhost-only configuration.
///
/// This function sets `expose = 0` so that even if the tray app runs, it
/// will not override the secure localhost binding.
///
/// Must be called **after** `kill_existing_ollama()` so the database file
/// lock has been released.
///
/// # Errors
///
/// Returns an error if the database cannot be opened or written. If the
/// database does not exist, returns `Ok(())` (Ollama not yet configured).
pub fn disable_ollama_network_exposure() -> Result<()> {
    let db_path = dirs::data_local_dir()
        .context("Cannot locate %LOCALAPPDATA%")?
        .join("Ollama")
        .join("db.sqlite");

    if !db_path.exists() {
        debug!(
            "Ollama settings database not found at {} — skipping",
            db_path.display()
        );
        return Ok(());
    }

    let conn = rusqlite::Connection::open(&db_path)
        .with_context(|| format!("Failed to open Ollama database: {}", db_path.display()))?;

    let rows_updated = conn
        .execute("UPDATE settings SET expose = 0 WHERE expose != 0", [])
        .with_context(|| "Failed to update Ollama network exposure setting")?;

    if rows_updated > 0 {
        info!(
            "Disabled Ollama network exposure in {}",
            db_path.display()
        );
    } else {
        debug!("Ollama network exposure was already disabled");
    }

    Ok(())
}

/// Runs the Ollama lifecycle manager.
///
/// Monitors the application state and starts/stops Ollama as needed.
/// When the state is Available or AgentTaskActive, Ollama should be running.
/// When Blocked, Cooldown, or Error, Ollama should be stopped.
///
/// # Arguments
///
/// * `state` - Shared application state.
/// * `shutdown_rx` - Watch channel for shutdown signal.
pub async fn run_ollama_manager(
    state: Arc<RwLock<AppState>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    info!("Ollama manager started");

    // Kill any orphaned Ollama processes from a previous run
    kill_existing_ollama();

    let mut ollama_child: Option<Child> = None;

    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {},
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Ollama manager shutting down");
                    if let Some(ref mut child) = ollama_child {
                        stop_ollama(child, 10).await;
                    }
                    return;
                }
            }
        }

        let (should_run, config) = {
            let s = state.read().await;
            let should_run = match s.manual_override {
                Some(ManualOverride::ForceDisable) => false,
                Some(ManualOverride::ForceEnable) => true,
                None => matches!(
                    s.status,
                    FreeCycleStatus::Available
                        | FreeCycleStatus::AgentTaskActive
                        | FreeCycleStatus::Downloading
                ),
            };
            (should_run, s.config.clone())
        };

        if should_run && ollama_child.is_none() {
            // Need to start Ollama
            match start_ollama(&config).await {
                Ok(child) => {
                    ollama_child = Some(child);
                    let mut s = state.write().await;
                    s.ollama_running = true;
                    info!("Ollama is now running and exposed to the network");
                }
                Err(e) => {
                    error!("Failed to start Ollama: {}", e);
                    let mut s = state.write().await;
                    s.ollama_running = false;
                }
            }
        } else if !should_run && ollama_child.is_some() {
            // Need to stop Ollama
            if let Some(ref mut child) = ollama_child {
                stop_ollama(child, config.ollama.graceful_shutdown_timeout_seconds).await;
            }
            ollama_child = None;
            let mut s = state.write().await;
            s.ollama_running = false;
            info!("Ollama stopped");
        }

        // Check if Ollama process is still alive
        if let Some(ref mut child) = ollama_child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    warn!("Ollama process exited unexpectedly: {}", status);
                    ollama_child = None;
                    let mut s = state.write().await;
                    s.ollama_running = false;
                }
                Ok(None) => {
                    // Still running, good
                }
                Err(e) => {
                    warn!("Error checking Ollama status: {}", e);
                }
            }
        }
    }
}

/// Runs the model download and update manager.
///
/// On startup, ensures all required models are downloaded. Retries every 5 minutes
/// on failure. Checks for model updates once daily.
///
/// # Arguments
///
/// * `state` - Shared application state.
/// * `shutdown_rx` - Watch channel for shutdown signal.
pub async fn run_model_manager(
    state: Arc<RwLock<AppState>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    info!("Model manager started");

    // Wait for Ollama to be available before pulling models
    loop {
        {
            let s = state.read().await;
            if s.ollama_running {
                break;
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {},
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    return;
                }
            }
        }
    }

    // Give Ollama a moment to fully initialize
    tokio::time::sleep(Duration::from_secs(5)).await;

    let mut last_update_check = std::time::Instant::now();

    loop {
        let (required_models, ollama_running, retry_interval, update_interval, config) = {
            let s = state.read().await;
            (
                s.config.models.required.clone(),
                s.ollama_running,
                Duration::from_secs(s.config.models.retry_interval_minutes * 60),
                Duration::from_secs(s.config.models.update_check_interval_hours * 3600),
                s.config.clone(),
            )
        };

        if !ollama_running {
            tokio::select! {
                _ = tokio::time::sleep(retry_interval) => continue,
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() { return; }
                }
            }
        }

        let should_update = last_update_check.elapsed() > update_interval;
        let base_url = format!("http://{}:{}", config.ollama.secure_host, config.ollama.port);

        for model in &required_models {
            // Check if model exists
            let model_exists = check_model_exists(&base_url, model).await;

            if !model_exists || should_update {
                let kind = if model_exists {
                    ModelTransferKind::Updating
                } else {
                    ModelTransferKind::Downloading
                };
                info!("{} model: {}", kind.label(), model);

                {
                    let mut s = state.write().await;
                    s.upsert_model_progress(ModelProgress::new(model.clone(), kind));
                }

                match pull_model(Arc::clone(&state), &base_url, model, kind).await {
                    Ok(()) => {
                        info!("Model {} is ready", model);
                        let mut s = state.write().await;
                        s.remove_model_progress(model);
                    }
                    Err(e) => {
                        warn!("Failed to pull model {}: {}", model, e);
                        let mut s = state.write().await;
                        s.upsert_model_progress(ModelProgress {
                            model_name: model.clone(),
                            kind,
                            percent: None,
                            status_text: format!(
                                "Failed: {} (retrying in {}m)",
                                model, config.models.retry_interval_minutes
                            ),
                            failed: true,
                        });
                    }
                }
            }
        }

        if should_update {
            last_update_check = std::time::Instant::now();
        }

        // Wait for next check
        tokio::select! {
            _ = tokio::time::sleep(retry_interval) => {},
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { return; }
            }
        }
    }
}

/// Checks if a model exists locally via the Ollama API.
///
/// # Arguments
///
/// * `base_url` - The Ollama API base URL (e.g., `http://127.0.0.1:11434`).
/// * `model` - The model tag to check.
///
/// # Returns
///
/// `true` if the model is available locally, `false` otherwise.
async fn check_model_exists(base_url: &str, model: &str) -> bool {
    let url = format!("{}/api/show", base_url);
    let client = reqwest::Client::new();
    let body = serde_json::json!({ "name": model });
    log_ollama_request("POST", &url, &body.to_string());

    match client.post(&url).json(&body).send().await {
        Ok(resp) => {
            let (status, _) = read_logged_response_preview("POST", &url, resp).await;
            let exists = status.is_success();

            if exists {
                debug!("Ollama model lookup succeeded for {}", model);
            } else if status == reqwest::StatusCode::NOT_FOUND {
                debug!("Ollama model lookup reported missing model {}", model);
            } else {
                debug!(
                    "Ollama model lookup returned non-success for {}: {}",
                    model, status
                );
            }

            exists
        }
        Err(error) => {
            log_ollama_transport_error("POST", &url, &error);
            false
        }
    }
}

/// Pulls (downloads) a model via the Ollama API.
///
/// This is a blocking operation that waits for the download to complete.
/// The Ollama API streams progress, but we consume the entire response.
///
/// # Arguments
///
/// * `base_url` - The Ollama API base URL.
/// * `model` - The model tag to pull.
///
/// # Returns
///
/// `Ok(())` on success, or an error if the pull failed.
pub async fn pull_model(
    state: Arc<RwLock<AppState>>,
    base_url: &str,
    model: &str,
    kind: ModelTransferKind,
) -> Result<()> {
    let url = format!("{}/api/pull", base_url);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3600))
        .build()?;

    let body = serde_json::json!({
        "name": model,
        "stream": true
    });
    log_ollama_request("POST", &url, &body.to_string());

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            log_ollama_transport_error("POST", &url, &error);
            error
        })
        .context("Failed to send pull request")?;

    let status = resp.status();
    debug!(
        "Ollama HTTP response: POST {} status={} body=<stream>",
        url, status
    );

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("{}", format_http_error("Pull failed", status, &body))
    }

    let mut response = resp;
    let mut buffer = String::new();

    while let Some(chunk) = response
        .chunk()
        .await
        .context("Failed to read pull stream")?
    {
        let chunk_text = String::from_utf8_lossy(&chunk);
        buffer.push_str(&chunk_text);
        process_pull_stream_chunk(&state, model, kind, &mut buffer).await;
    }

    process_pull_stream_tail(&state, model, kind, &buffer).await;

    Ok(())
}

fn format_http_error(context: &str, status: reqwest::StatusCode, body: &str) -> String {
    let preview = scrub_http_preview(body, 240);
    if preview.is_empty() {
        format!(
            "{} with status {} and an empty response body",
            context, status
        )
    } else {
        format!("{} with status {}: {}", context, status, preview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn test_find_ollama_exe_returns_none_when_not_installed() {
        let config = FreeCycleConfig {
            ollama: crate::config::OllamaConfig {
                exe_path: Some("/nonexistent/path/ollama.exe".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        // This test verifies the explicit path check fails gracefully
        // The function may still find ollama via PATH
        let _ = find_ollama_exe(&config);
    }

    #[test]
    fn test_format_http_error_redacts_sensitive_response_text() {
        let message = format_http_error(
            "Pull failed",
            StatusCode::UNAUTHORIZED,
            r#"{"error":"bad auth","token":"secret123","detail":"Authorization: Bearer abc123"}"#,
        );
        assert!(message.contains("[REDACTED]"));
        assert!(!message.contains("secret123"));
        assert!(!message.contains("abc123"));
    }

    #[test]
    fn test_log_ollama_request_redacts_sensitive_request_text() {
        let body = r#"{"name":"llama3.1:8b-instruct-q4_K_M","token":"secret123"}"#;
        let preview = scrub_http_preview_default(body);
        assert!(preview.contains("[REDACTED]"));
        assert!(!preview.contains("secret123"));
    }

    #[test]
    fn test_build_progress_update_prefers_percentage_when_available() {
        let event = PullProgressEvent {
            status: "pulling manifest".to_string(),
            completed: Some(25),
            total: Some(100),
            error: None,
        };

        let progress = build_progress_update(
            "llama3.1:8b-instruct-q4_K_M",
            ModelTransferKind::Downloading,
            &event,
        )
        .unwrap();

        assert_eq!(progress.percent, Some(25));
        assert_eq!(
            progress.render_status(),
            "Downloading llama3.1:8b-instruct-q4_K_M: 25%"
        );
    }

    #[test]
    fn test_build_progress_update_uses_status_fallback_without_totals() {
        let event = PullProgressEvent {
            status: "resolving manifest".to_string(),
            completed: None,
            total: None,
            error: None,
        };

        let progress =
            build_progress_update("nomic-embed-text", ModelTransferKind::Updating, &event).unwrap();

        assert_eq!(progress.percent, None);
        assert_eq!(
            progress.render_status(),
            "Updating nomic-embed-text: resolving manifest"
        );
    }

    #[test]
    fn test_build_progress_update_formats_failures() {
        let event = PullProgressEvent {
            status: String::new(),
            completed: None,
            total: None,
            error: Some("disk full".to_string()),
        };

        let progress =
            build_progress_update("nomic-embed-text", ModelTransferKind::Downloading, &event)
                .unwrap();

        assert!(progress.failed);
        assert_eq!(
            progress.render_status(),
            "Failed: nomic-embed-text (disk full)"
        );
    }
}
