//! Logging initialization and credential scrubbing for FreeCycle.
//!
//! When verbose mode (`-v`) is active, all log output is written to
//! `~/freecycle-verbose.log` (truncated at start) and to stderr.
//! When not verbose, only warnings and errors go to stderr.
//!
//! All log output passes through credential scrubbing to ensure no
//! sensitive values (tokens, keys, passwords) are ever written to logs.

use anyhow::{Context, Result};
use regex_lite::Regex;
use std::path::PathBuf;
use std::sync::OnceLock;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

const DEFAULT_HTTP_PREVIEW_CHARS: usize = 160;
const SENSITIVE_FIELD_PATTERN: &str = concat!(
    "(?:",
    "token",
    "|access[_-]?token",
    "|refresh[_-]?token",
    "|id[_-]?token",
    "|api[_-]?key",
    "|x[_-]?api[_-]?key",
    "|apikey",
    "|secret",
    "|client[_-]?secret",
    "|password",
    "|passphrase",
    "|credential",
    "|credentials",
    "|authorization",
    "|cookie",
    "|set[_-]?cookie",
    "|session(?:[_-]?token)?",
    "|bearer",
    ")"
);

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
    let mut result = input.to_string();
    result = header_regex()
        .replace_all(&result, "$1$2: [REDACTED]")
        .into_owned();
    result = json_double_quote_regex()
        .replace_all(&result, "$1: \"[REDACTED]\"")
        .into_owned();
    result = json_single_quote_regex()
        .replace_all(&result, "$1: '[REDACTED]'")
        .into_owned();
    result = form_field_regex()
        .replace_all(&result, "$1=[REDACTED]")
        .into_owned();
    bearer_regex()
        .replace_all(&result, "Bearer [REDACTED]")
        .into_owned()
}

/// Scrubs and truncates arbitrary HTTP-derived text so it is safe to log.
pub fn scrub_http_preview(input: &str, max_chars: usize) -> String {
    let scrubbed = scrub_credentials(input);
    let normalized = normalize_whitespace(&scrubbed);
    truncate_chars(&normalized, max_chars)
}

/// Scrubs and truncates HTTP-derived text using the default preview limit.
pub fn scrub_http_preview_default(input: &str) -> String {
    scrub_http_preview(input, DEFAULT_HTTP_PREVIEW_CHARS)
}

fn header_regex() -> &'static Regex {
    static HEADER_REGEX: OnceLock<Regex> = OnceLock::new();
    HEADER_REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)(^|(?:\r?\n))(\s*(?:authorization|cookie|set-cookie|x-api-key))\s*:\s*[^\r\n]*",
        )
        .expect("valid header scrub regex")
    })
}

fn json_double_quote_regex() -> &'static Regex {
    static JSON_DOUBLE_QUOTE_REGEX: OnceLock<Regex> = OnceLock::new();
    JSON_DOUBLE_QUOTE_REGEX.get_or_init(|| {
        Regex::new(&format!(
            r#"(?i)("(?:{})")\s*:\s*"[^"]*""#,
            SENSITIVE_FIELD_PATTERN
        ))
        .expect("valid JSON double-quoted scrub regex")
    })
}

fn json_single_quote_regex() -> &'static Regex {
    static JSON_SINGLE_QUOTE_REGEX: OnceLock<Regex> = OnceLock::new();
    JSON_SINGLE_QUOTE_REGEX.get_or_init(|| {
        Regex::new(&format!(
            r#"(?i)('(?:{})')\s*:\s*'[^']*'"#,
            SENSITIVE_FIELD_PATTERN
        ))
        .expect("valid JSON single-quoted scrub regex")
    })
}

fn form_field_regex() -> &'static Regex {
    static FORM_FIELD_REGEX: OnceLock<Regex> = OnceLock::new();
    FORM_FIELD_REGEX.get_or_init(|| {
        Regex::new(&format!(
            r"(?i)\b({})\s*=\s*([^\s&]+)",
            SENSITIVE_FIELD_PATTERN
        ))
        .expect("valid form-field scrub regex")
    })
}

fn bearer_regex() -> &'static Regex {
    static BEARER_REGEX: OnceLock<Regex> = OnceLock::new();
    BEARER_REGEX
        .get_or_init(|| Regex::new(r"(?i)\bBearer\s+[^\s,;]+").expect("valid bearer scrub regex"))
}

fn normalize_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let mut end_index = input.len();
    let mut char_count = 0;

    for (index, _) in input.char_indices() {
        if char_count == max_chars {
            end_index = index;
            break;
        }
        char_count += 1;
    }

    if char_count == max_chars && end_index < input.len() {
        let mut truncated = input[..end_index].to_string();
        truncated.push_str("...");
        truncated
    } else {
        input.to_string()
    }
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
    fn test_scrub_credentials_headers_case_insensitive() {
        let input = "authorization: Bearer token123\r\nCookie: session=abc\r\nSet-Cookie: auth=xyz\r\nX-Api-Key: key123";
        let scrubbed = scrub_credentials(input);
        assert!(scrubbed.contains("authorization: [REDACTED]"));
        assert!(scrubbed.contains("Cookie: [REDACTED]"));
        assert!(scrubbed.contains("Set-Cookie: [REDACTED]"));
        assert!(scrubbed.contains("X-Api-Key: [REDACTED]"));
        assert!(!scrubbed.contains("token123"));
        assert!(!scrubbed.contains("session=abc"));
        assert!(!scrubbed.contains("auth=xyz"));
        assert!(!scrubbed.contains("key123"));
    }

    #[test]
    fn test_scrub_credentials_json_sensitive_variants() {
        let input =
            r#"{"API_KEY":"secret-1","client_secret":"secret-2","access_token":"secret-3"}"#;
        let scrubbed = scrub_credentials(input);
        assert!(scrubbed.contains(r#""API_KEY": "[REDACTED]""#));
        assert!(scrubbed.contains(r#""client_secret": "[REDACTED]""#));
        assert!(scrubbed.contains(r#""access_token": "[REDACTED]""#));
        assert!(!scrubbed.contains("secret-1"));
        assert!(!scrubbed.contains("secret-2"));
        assert!(!scrubbed.contains("secret-3"));
    }

    #[test]
    fn test_scrub_credentials_form_encoded_fields() {
        let input = "token=secret123&name=test&api_key=secret456";
        let scrubbed = scrub_credentials(input);
        assert!(scrubbed.contains("token=[REDACTED]"));
        assert!(scrubbed.contains("api_key=[REDACTED]"));
        assert!(!scrubbed.contains("secret123"));
        assert!(!scrubbed.contains("secret456"));
    }

    #[test]
    fn test_scrub_http_preview_truncates_after_redaction() {
        let input = format!(r#"{{"token":"secret123","details":"{}"}}"#, "a".repeat(300));
        let preview = scrub_http_preview(&input, 80);
        assert!(preview.contains("[REDACTED]"));
        assert!(!preview.contains("secret123"));
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn test_scrub_credentials_no_sensitive_data() {
        let input = r#"{"name": "test", "count": 42}"#;
        let scrubbed = scrub_credentials(input);
        assert_eq!(scrubbed, input);
    }
}
