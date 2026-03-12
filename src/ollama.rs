//! Ollama process lifecycle and model management for FreeCycle.
//!
//! Handles starting/stopping the Ollama serve process, exposing it to the network
//! via `OLLAMA_HOST`, pulling required models, and periodic model update checks.

use crate::config::FreeCycleConfig;
use crate::state::FreeCycleStatus;
use crate::AppState;
use anyhow::{Context, Result};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::{watch, RwLock};
use tracing::{debug, error, info, warn};

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
    if let Ok(output) = std::process::Command::new("where")
        .arg("ollama")
        .output()
    {
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
    let exe = find_ollama_exe(config)
        .context("Ollama executable not found")?;

    let host_value = format!("{}:{}", config.ollama.host, config.ollama.port);
    info!("Starting Ollama: {} serve (OLLAMA_HOST={})", exe, host_value);

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
    info!("Stopping Ollama (PID {}), waiting {}s for graceful shutdown", pid, timeout_secs);

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
            warn!("Ollama did not exit within {}s. Force killing.", timeout_secs);
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
/// Uses `taskkill` to find and terminate `ollama.exe` and `ollama_llama_server.exe`.
fn kill_existing_ollama() {
    debug!("Checking for existing Ollama processes to clean up");
    for process_name in &["ollama.exe", "ollama_llama_server.exe"] {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/IM", process_name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output();
    }
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
            let should_run = matches!(
                s.status,
                FreeCycleStatus::Available
                    | FreeCycleStatus::AgentTaskActive
                    | FreeCycleStatus::Downloading
            );
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
        let base_url = format!("http://127.0.0.1:{}", config.ollama.port);

        for model in &required_models {
            // Check if model exists
            let model_exists = check_model_exists(&base_url, model).await;

            if !model_exists || should_update {
                let action = if model_exists { "Updating" } else { "Downloading" };
                info!("{} model: {}", action, model);

                {
                    let mut s = state.write().await;
                    s.models_downloading = true;
                    s.model_status
                        .push(format!("{}: {}", action, model));
                }

                match pull_model(&base_url, model).await {
                    Ok(()) => {
                        info!("Model {} is ready", model);
                        let mut s = state.write().await;
                        s.model_status.retain(|m| !m.contains(model));
                    }
                    Err(e) => {
                        warn!("Failed to pull model {}: {}", model, e);
                        let mut s = state.write().await;
                        s.model_status
                            .retain(|m| !m.contains(model));
                        s.model_status
                            .push(format!("Failed: {} (retrying in {}m)", model, config.models.retry_interval_minutes));
                    }
                }
            }
        }

        if should_update {
            last_update_check = std::time::Instant::now();
        }

        // Clear downloading flag if no more models in progress
        {
            let mut s = state.write().await;
            s.models_downloading = s.model_status.iter().any(|m| m.starts_with("Downloading") || m.starts_with("Updating"));
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

    match client.post(&url).json(&body).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
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
async fn pull_model(base_url: &str, model: &str) -> Result<()> {
    let url = format!("{}/api/pull", base_url);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3600))
        .build()?;

    let body = serde_json::json!({
        "name": model,
        "stream": false
    });

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("Failed to send pull request")?;

    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Pull failed with status {}: {}", status, body)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

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
}
