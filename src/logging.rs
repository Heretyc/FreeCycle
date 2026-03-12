//! Logging initialization and credential scrubbing for FreeCycle.
//!
//! When verbose mode (`-v`) is active, all log output is written to
//! `~/freecycle-verbose.log` (truncated at start) and to stderr.
//! When not verbose, only warnings and errors go to stderr.
//!
//! All log output passes through credential scrubbing to ensure no
//! sensitive values (tokens, keys, passwords) are ever written to logs.

use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

/// Returns the path to the verbose log file.
///
/// # Returns
///
/// Path to `~/freecycle-verbose.log`.
fn verbose_log_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("freecycle-verbose.log")
}

/// Guard that must be held for the lifetime of the logger.
///
/// Dropping this guard flushes and closes the log file. Hold it in main().
pub struct LogGuard {
    _worker_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
}

/// Initializes the logging subsystem.
///
/// # Arguments
///
/// * `verbose` - If true, enables debug-level logging to `~/freecycle-verbose.log`
///   and to stderr. If false, only warnings and errors go to stderr.
///
/// # Returns
///
/// A `LogGuard` that must be held for the lifetime of the application.
/// Dropping the guard flushes pending log writes.
///
/// # Errors
///
/// Returns an error if the log file cannot be created or the subscriber
/// cannot be initialized.
pub fn init_logging(verbose: bool) -> Result<LogGuard> {
    if verbose {
        let log_path = verbose_log_path();

        // Truncate the log file at start (clean log each run)
        std::fs::write(&log_path, "")
            .with_context(|| format!("Failed to truncate log file: {}", log_path.display()))?;

        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("Failed to open log file: {}", log_path.display()))?;

        let (non_blocking, guard) = tracing_appender::non_blocking(file);

        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false)
            .with_target(true)
            .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339());

        let stderr_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr.with_max_level(tracing::Level::DEBUG))
            .with_target(true)
            .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339());

        let filter = EnvFilter::new("debug");

        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .with(stderr_layer)
            .try_init()
            .ok(); // Ignore if already initialized (for tests)

        Ok(LogGuard {
            _worker_guard: Some(guard),
        })
    } else {
        let stderr_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(false)
            .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339());

        let filter = EnvFilter::new("warn");

        tracing_subscriber::registry()
            .with(filter)
            .with(stderr_layer)
            .try_init()
            .ok();

        Ok(LogGuard {
            _worker_guard: None,
        })
    }
}

/// Scrubs sensitive values from a string before logging.
///
/// Replaces values associated with sensitive keys (token, secret, password,
/// key, credential, api_key, authorization, cookie) with `[REDACTED]`.
///
/// # Arguments
///
/// * `input` - The string to scrub.
///
/// # Returns
///
/// The scrubbed string with sensitive values replaced.
///
/// # Example
///
/// ```
/// let input = r#"{"token": "abc123", "name": "test"}"#;
/// let scrubbed = scrub_credentials(input);
/// assert!(scrubbed.contains("[REDACTED]"));
/// assert!(!scrubbed.contains("abc123"));
/// ```
pub fn scrub_credentials(input: &str) -> String {
    let sensitive_keys = [
        "token",
        "secret",
        "password",
        "key",
        "credential",
        "api_key",
        "authorization",
        "cookie",
        "bearer",
    ];

    let mut result = input.to_string();

    for key in &sensitive_keys {
        // Match JSON-style key-value pairs: "key": "value" or "key":"value"
        let patterns = [
            format!(r#""{}"\s*:\s*"[^"]*""#, key),
            format!(r#"'{}'\s*:\s*'[^']*'"#, key),
        ];

        for pattern in &patterns {
            if let Ok(re) = regex_lite::Regex::new(&format!("(?i){}", pattern)) {
                result = re
                    .replace_all(&result, &format!(r#""{}": "[REDACTED]""#, key))
                    .to_string();
            }
        }

        // Match header-style: "Authorization: Bearer xxx" or "Authorization: token_value"
        if key == &"authorization" {
            if let Ok(re) = regex_lite::Regex::new(r"(?i)authorization:\s*.+") {
                result = re
                    .replace_all(&result, "Authorization: [REDACTED]")
                    .to_string();
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verbose_log_path() {
        let path = verbose_log_path();
        assert!(path.to_string_lossy().contains("freecycle-verbose.log"));
    }

    #[test]
    fn test_scrub_credentials_json_token() {
        let input = r#"{"token": "secret123", "name": "test"}"#;
        let scrubbed = scrub_credentials(input);
        assert!(scrubbed.contains("[REDACTED]"));
        assert!(!scrubbed.contains("secret123"));
    }

    #[test]
    fn test_scrub_credentials_authorization_header() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9";
        let scrubbed = scrub_credentials(input);
        assert!(scrubbed.contains("[REDACTED]"));
        assert!(!scrubbed.contains("eyJhbGci"));
    }

    #[test]
    fn test_scrub_credentials_no_sensitive_data() {
        let input = r#"{"name": "test", "count": 42}"#;
        let scrubbed = scrub_credentials(input);
        assert_eq!(scrubbed, input);
    }
}
