//! Configuration management for FreeCycle.
//!
//! Loads and saves the TOML configuration file from `%APPDATA%\FreeCycle\config.toml`.
//! If no configuration file exists, creates one with sensible defaults.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::info;

/// Returns the path to the FreeCycle configuration directory.
///
/// On Windows, this is `%APPDATA%\FreeCycle\`.
///
/// # Returns
///
/// The path to the configuration directory.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("FreeCycle")
}

/// Returns the path to the FreeCycle configuration file.
///
/// # Returns
///
/// The path to `config.toml` within the configuration directory.
pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Top-level configuration for FreeCycle.
///
/// Deserialized from `%APPDATA%\FreeCycle\config.toml`. All fields have sensible
/// defaults so the program can run without a configuration file.
///
/// # Example
///
/// ```toml
/// [general]
/// gpu_check_interval_ms = 5000
/// tray_update_interval_ms = 2000
/// cooldown_seconds = 1800
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreeCycleConfig {
    /// General operational settings.
    #[serde(default)]
    pub general: GeneralConfig,

    /// Ollama process management settings.
    #[serde(default)]
    pub ollama: OllamaConfig,

    /// Model download and update settings.
    #[serde(default)]
    pub models: ModelConfig,

    /// Processes that trigger GPU unavailability.
    #[serde(default = "default_blacklisted_processes")]
    pub blacklisted_processes: ProcessList,

    /// Processes exempt from VRAM/GPU usage checks.
    #[serde(default = "default_whitelisted_processes")]
    pub whitelisted_processes: ProcessList,

    /// Agent signal server settings.
    #[serde(default)]
    pub agent_server: AgentServerConfig,

    /// Security configuration for cryptographic and identity settings.
    #[serde(default)]
    pub security: SecurityConfig,
}

/// General operational timing and threshold settings.
///
/// # Fields
///
/// * `gpu_check_interval_ms` - How often to check GPU status (default: 5000ms).
/// * `tray_update_interval_ms` - How often to update the tray icon (default: 2000ms).
/// * `cooldown_seconds` - Cooldown period after a blacklisted process exits (default: 1800s).
/// * `vram_threshold_percent` - VRAM usage percent from non-whitelisted processes that blocks (default: 50).
/// * `vram_idle_mb` - VRAM usage below this (in MB) is considered idle for agent task tracking (default: 300).
/// * `vram_idle_timeout_minutes` - Minutes of idle VRAM before agent task is cleared (default: 3).
/// * `task_timeout_hours` - Hours of no activity before agent task assumption expires (default: 1).
/// * `wake_delay_seconds` - Seconds to wait after system wake before re-enabling Ollama (default: 60).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_gpu_check_interval")]
    pub gpu_check_interval_ms: u64,

    #[serde(default = "default_tray_update_interval")]
    pub tray_update_interval_ms: u64,

    #[serde(default = "default_cooldown_seconds")]
    pub cooldown_seconds: u64,

    #[serde(default = "default_vram_threshold_percent")]
    pub vram_threshold_percent: u64,

    #[serde(default = "default_vram_idle_mb")]
    pub vram_idle_mb: u64,

    #[serde(default = "default_vram_idle_timeout_minutes")]
    pub vram_idle_timeout_minutes: u64,

    #[serde(default = "default_task_timeout_hours")]
    pub task_timeout_hours: u64,

    #[serde(default = "default_wake_delay_seconds")]
    pub wake_delay_seconds: u64,
}

/// Ollama process and network configuration.
///
/// # Fields
///
/// * `host` - The host address Ollama binds to in compatibility mode (default: "0.0.0.0"). Ignored in secure mode.
/// * `secure_host` - The loopback address Ollama binds to in secure mode (default: "127.0.0.1").
///   Change to e.g. "127.0.0.2" to prevent other local applications from discovering Ollama
///   on the standard `localhost`/`127.0.0.1` address. All 127.x.x.x addresses are valid
///   loopback addresses on Windows and require no additional network configuration.
/// * `port` - The port Ollama listens on (default: 11434).
/// * `graceful_shutdown_timeout_seconds` - Seconds to wait for graceful shutdown before force kill (default: 10).
/// * `exe_path` - Optional explicit path to the ollama executable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaConfig {
    #[serde(default = "default_ollama_host")]
    pub host: String,

    #[serde(default = "default_ollama_secure_host")]
    pub secure_host: String,

    #[serde(default = "default_ollama_port")]
    pub port: u16,

    #[serde(default = "default_graceful_shutdown_timeout")]
    pub graceful_shutdown_timeout_seconds: u64,

    /// Optional explicit path to the ollama executable. If not set, FreeCycle
    /// searches PATH and common install locations.
    #[serde(default)]
    pub exe_path: Option<String>,
}

/// Model download and update configuration.
///
/// # Fields
///
/// * `required` - List of model tags that must be present and kept updated.
/// * `retry_interval_minutes` - Minutes between download retry attempts on failure (default: 5).
/// * `update_check_interval_hours` - Hours between update checks for already-downloaded models (default: 24).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(default = "default_required_models")]
    pub required: Vec<String>,

    #[serde(default = "default_retry_interval")]
    pub retry_interval_minutes: u64,

    #[serde(default = "default_update_check_interval")]
    pub update_check_interval_hours: u64,
}

/// A list of process names.
///
/// Used for both blacklisted (game) and whitelisted (exempt) process lists.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessList {
    #[serde(default)]
    pub list: Vec<String>,
}

/// Agent signal server configuration.
///
/// # Fields
///
/// * `port` - The port the agent signal HTTP server listens on (default: 7443).
/// * `bind_address` - The address to bind to (default: "0.0.0.0").
/// * `compatibility_mode` - If false (default), secure mode is active (TLS, Ollama bound to 127.0.0.1).
///   If true, plaintext HTTP and Ollama exposed on 0.0.0.0 (legacy behavior).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentServerConfig {
    #[serde(default = "default_agent_port")]
    pub port: u16,

    #[serde(default = "default_agent_bind")]
    pub bind_address: String,

    #[serde(default = "default_compatibility_mode")]
    pub compatibility_mode: bool,
}

/// Security configuration for keypair, certificate, and identity settings.
///
/// # Fields
///
/// * `keypair_path` - Optional path to Ed25519 keypair files (default: alongside config.toml).
/// * `cert_path` - Optional path to TLS certificate file (default: alongside config.toml).
/// * `identity_uuid` - Optional UUID for server identity (default: auto-generated on first run).
/// * `fingerprint_override` - Optional manual GPU fingerprint override (default: derived from NVML).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityConfig {
    #[serde(default)]
    pub keypair_path: Option<String>,

    #[serde(default)]
    pub cert_path: Option<String>,

    #[serde(default)]
    pub identity_uuid: Option<String>,

    #[serde(default)]
    pub fingerprint_override: Option<String>,
}

// Default value functions for serde

fn default_gpu_check_interval() -> u64 {
    5000
}
fn default_tray_update_interval() -> u64 {
    2000
}
fn default_cooldown_seconds() -> u64 {
    1800
}
fn default_vram_threshold_percent() -> u64 {
    50
}
fn default_vram_idle_mb() -> u64 {
    300
}
fn default_vram_idle_timeout_minutes() -> u64 {
    3
}
fn default_task_timeout_hours() -> u64 {
    1
}
fn default_wake_delay_seconds() -> u64 {
    60
}
fn default_ollama_host() -> String {
    "0.0.0.0".to_string()
}
fn default_ollama_secure_host() -> String {
    "127.0.0.1".to_string()
}
fn default_ollama_port() -> u16 {
    11434
}
fn default_graceful_shutdown_timeout() -> u64 {
    10
}
fn default_agent_port() -> u16 {
    7443
}
fn default_agent_bind() -> String {
    "0.0.0.0".to_string()
}
fn default_compatibility_mode() -> bool {
    false
}

fn default_required_models() -> Vec<String> {
    vec![
        "llama3.1:8b-instruct-q4_K_M".to_string(),
        "nomic-embed-text".to_string(),
    ]
}

fn default_blacklisted_processes() -> ProcessList {
    ProcessList {
        list: vec![
            "VRChat.exe".to_string(),
            "vrchat.exe".to_string(),
            "Cyberpunk2077.exe".to_string(),
            "HELLDIVERS2.exe".to_string(),
            "GenshinImpact.exe".to_string(),
            "ZenlessZoneZero.exe".to_string(),
            "Overwatch.exe".to_string(),
            "VALORANT.exe".to_string(),
            "eldenring.exe".to_string(),
            "MonsterHunterWilds.exe".to_string(),
        ],
    }
}

fn default_whitelisted_processes() -> ProcessList {
    ProcessList {
        list: vec![
            "ollama_llama_server".to_string(),
            "ollama_llama_server.exe".to_string(),
            "ollama.exe".to_string(),
            "ollama".to_string(),
            "dwm.exe".to_string(),
            "csrss.exe".to_string(),
        ],
    }
}

fn default_retry_interval() -> u64 {
    5
}
fn default_update_check_interval() -> u64 {
    24
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            gpu_check_interval_ms: default_gpu_check_interval(),
            tray_update_interval_ms: default_tray_update_interval(),
            cooldown_seconds: default_cooldown_seconds(),
            vram_threshold_percent: default_vram_threshold_percent(),
            vram_idle_mb: default_vram_idle_mb(),
            vram_idle_timeout_minutes: default_vram_idle_timeout_minutes(),
            task_timeout_hours: default_task_timeout_hours(),
            wake_delay_seconds: default_wake_delay_seconds(),
        }
    }
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            host: default_ollama_host(),
            secure_host: default_ollama_secure_host(),
            port: default_ollama_port(),
            graceful_shutdown_timeout_seconds: default_graceful_shutdown_timeout(),
            exe_path: None,
        }
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            required: default_required_models(),
            retry_interval_minutes: default_retry_interval(),
            update_check_interval_hours: default_update_check_interval(),
        }
    }
}

impl Default for AgentServerConfig {
    fn default() -> Self {
        Self {
            port: default_agent_port(),
            bind_address: default_agent_bind(),
            compatibility_mode: default_compatibility_mode(),
        }
    }
}

impl Default for FreeCycleConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            ollama: OllamaConfig::default(),
            models: ModelConfig::default(),
            blacklisted_processes: default_blacklisted_processes(),
            whitelisted_processes: default_whitelisted_processes(),
            agent_server: AgentServerConfig::default(),
            security: SecurityConfig::default(),
        }
    }
}

impl FreeCycleConfig {
    /// Loads the configuration from disk, or creates a default one if it does not exist.
    ///
    /// The configuration file is located at `%APPDATA%\FreeCycle\config.toml`.
    /// If the file does not exist, a default configuration is written to disk
    /// and returned.
    ///
    /// # Returns
    ///
    /// The loaded (or newly created default) configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the config file exists but cannot be read or parsed,
    /// or if the default config cannot be written to disk.
    pub fn load_or_create_default() -> Result<Self> {
        let path = config_path();

        if path.exists() {
            let contents = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read config file: {}", path.display()))?;
            let config: Self = toml::from_str(&contents)
                .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
            Ok(config)
        } else {
            let config = Self::default();
            config.save()?;
            info!("Created default configuration at {}", path.display());
            Ok(config)
        }
    }

    /// Saves the current configuration to disk.
    ///
    /// Creates the configuration directory if it does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be created or the file cannot be written.
    pub fn save(&self) -> Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create config directory: {}", dir.display()))?;

        let path = config_path();
        let contents = toml::to_string_pretty(self).context("Failed to serialize configuration")?;
        std::fs::write(&path, contents)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_serialization() {
        let config = FreeCycleConfig::default();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: FreeCycleConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.general.gpu_check_interval_ms, 5000);
        assert_eq!(deserialized.general.cooldown_seconds, 1800);
        assert_eq!(deserialized.ollama.port, 11434);
        assert_eq!(deserialized.models.required.len(), 2);
        assert_eq!(deserialized.blacklisted_processes.list.len(), 10);
        assert_eq!(deserialized.whitelisted_processes.list.len(), 6);
    }

    #[test]
    fn test_partial_config_deserialization() {
        let partial = r#"
[general]
cooldown_seconds = 3600

[ollama]
port = 8080
"#;
        let config: FreeCycleConfig = toml::from_str(partial).unwrap();
        assert_eq!(config.general.cooldown_seconds, 3600);
        assert_eq!(config.general.gpu_check_interval_ms, 5000); // default
        assert_eq!(config.ollama.port, 8080);
        assert_eq!(config.ollama.host, "0.0.0.0"); // default
    }

    #[test]
    fn test_nested_sections_backfill_missing_fields() {
        let partial = r#"
[general]
cooldown_seconds = 42

[ollama]
host = "127.0.0.1"
"#;

        let config: FreeCycleConfig = toml::from_str(partial).unwrap();
        let defaults = FreeCycleConfig::default();

        assert_eq!(config.general.cooldown_seconds, 42);
        assert_eq!(
            config.general.gpu_check_interval_ms,
            defaults.general.gpu_check_interval_ms
        );
        assert_eq!(
            config.general.tray_update_interval_ms,
            defaults.general.tray_update_interval_ms
        );
        assert_eq!(config.ollama.host, "127.0.0.1");
        assert_eq!(config.ollama.port, defaults.ollama.port);
        assert_eq!(
            config.ollama.graceful_shutdown_timeout_seconds,
            defaults.ollama.graceful_shutdown_timeout_seconds
        );
        assert_eq!(config.models.required, defaults.models.required);
        assert_eq!(
            config.blacklisted_processes.list,
            defaults.blacklisted_processes.list
        );
        assert_eq!(
            config.whitelisted_processes.list,
            defaults.whitelisted_processes.list
        );
        assert_eq!(config.agent_server.port, defaults.agent_server.port);
        assert_eq!(
            config.agent_server.bind_address,
            defaults.agent_server.bind_address
        );
    }

    #[test]
    fn test_ollama_exe_path_round_trip_and_none_omission() {
        let mut config = FreeCycleConfig::default();
        config.ollama.exe_path = Some(r"C:\Program Files\Ollama\ollama.exe".to_string());

        let serialized = toml::to_string_pretty(&config).unwrap();
        assert!(serialized.contains("exe_path = "));
        assert!(serialized.contains("Program Files"));
        assert!(serialized.contains("ollama.exe"));

        let deserialized: FreeCycleConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(
            deserialized.ollama.exe_path.as_deref(),
            Some(r"C:\Program Files\Ollama\ollama.exe")
        );
        assert_eq!(deserialized.ollama.port, config.ollama.port);
        assert_eq!(
            deserialized.general.cooldown_seconds,
            config.general.cooldown_seconds
        );

        let without_exe_path = toml::to_string_pretty(&FreeCycleConfig::default()).unwrap();
        assert!(!without_exe_path.contains("exe_path"));
    }

    #[test]
    fn test_empty_process_lists_override_defaults() {
        let config: FreeCycleConfig = toml::from_str(
            r#"
[blacklisted_processes]
list = []

[whitelisted_processes]
list = []
"#,
        )
        .unwrap();

        assert!(config.blacklisted_processes.list.is_empty());
        assert!(config.whitelisted_processes.list.is_empty());
    }

    #[test]
    fn test_invalid_config_types_are_rejected() {
        let invalid_numeric_type = r#"
[general]
cooldown_seconds = "3600"
"#;
        assert!(toml::from_str::<FreeCycleConfig>(invalid_numeric_type).is_err());

        let invalid_negative_number = r#"
[agent_server]
port = -1
"#;
        assert!(toml::from_str::<FreeCycleConfig>(invalid_negative_number).is_err());

        let invalid_scalar_for_array = r#"
[models]
required = "llama3.1:8b-instruct-q4_K_M"
"#;
        assert!(toml::from_str::<FreeCycleConfig>(invalid_scalar_for_array).is_err());
    }

    #[test]
    fn test_partial_models_and_agent_server_tables_use_defaults() {
        let partial = r#"
[models]
retry_interval_minutes = 15

[agent_server]
bind_address = "127.0.0.1"
"#;

        let config: FreeCycleConfig = toml::from_str(partial).unwrap();
        let defaults = FreeCycleConfig::default();

        assert_eq!(config.models.retry_interval_minutes, 15);
        assert_eq!(
            config.models.update_check_interval_hours,
            defaults.models.update_check_interval_hours
        );
        assert_eq!(config.models.required, defaults.models.required);
        assert_eq!(config.agent_server.bind_address, "127.0.0.1");
        assert_eq!(config.agent_server.port, defaults.agent_server.port);
    }

    #[test]
    fn test_config_dir_exists() {
        let dir = config_dir();
        assert!(dir.to_string_lossy().contains("FreeCycle"));
    }

    #[test]
    fn test_compatibility_mode_defaults_to_false() {
        let partial = r#"
[agent_server]
"#;
        let config: FreeCycleConfig = toml::from_str(partial).unwrap();
        assert_eq!(config.agent_server.compatibility_mode, false);
    }

    #[test]
    fn test_compatibility_mode_round_trip() {
        let mut config = FreeCycleConfig::default();
        config.agent_server.compatibility_mode = true;

        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: FreeCycleConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized.agent_server.compatibility_mode, true);
        assert_eq!(deserialized.agent_server.port, config.agent_server.port);
        assert_eq!(
            deserialized.agent_server.bind_address,
            config.agent_server.bind_address
        );
    }

    #[test]
    fn test_security_section_defaults() {
        // Empty [security] block should deserialize to all None.
        let empty_security = r#"
[security]
"#;
        let config: FreeCycleConfig = toml::from_str(empty_security).unwrap();
        assert_eq!(config.security.keypair_path, None);
        assert_eq!(config.security.cert_path, None);
        assert_eq!(config.security.identity_uuid, None);
        assert_eq!(config.security.fingerprint_override, None);

        // Completely missing security section should also deserialize to all None.
        let no_security = r#"
[general]
"#;
        let config: FreeCycleConfig = toml::from_str(no_security).unwrap();
        assert_eq!(config.security.keypair_path, None);
        assert_eq!(config.security.cert_path, None);
        assert_eq!(config.security.identity_uuid, None);
        assert_eq!(config.security.fingerprint_override, None);
    }

    #[test]
    fn test_security_section_round_trip() {
        let mut config = FreeCycleConfig::default();
        config.security.keypair_path = Some("/etc/freecycle/keypair.pem".to_string());
        config.security.cert_path = Some("/etc/freecycle/cert.pem".to_string());
        config.security.identity_uuid = Some("550e8400-e29b-41d4-a716-446655440000".to_string());
        config.security.fingerprint_override = Some("gpu-fingerprint-123".to_string());

        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: FreeCycleConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(
            deserialized.security.keypair_path,
            Some("/etc/freecycle/keypair.pem".to_string())
        );
        assert_eq!(
            deserialized.security.cert_path,
            Some("/etc/freecycle/cert.pem".to_string())
        );
        assert_eq!(
            deserialized.security.identity_uuid,
            Some("550e8400-e29b-41d4-a716-446655440000".to_string())
        );
        assert_eq!(
            deserialized.security.fingerprint_override,
            Some("gpu-fingerprint-123".to_string())
        );
    }

    #[test]
    fn test_security_keypair_path_none_omitted() {
        // Default config with all security fields None should not emit those keys when serialized.
        let config = FreeCycleConfig::default();
        let serialized = toml::to_string_pretty(&config).unwrap();

        // Verify that None values are not serialized as explicit keys.
        assert!(!serialized.contains("keypair_path"));
        assert!(!serialized.contains("cert_path"));
        assert!(!serialized.contains("identity_uuid"));
        assert!(!serialized.contains("fingerprint_override"));
    }

    #[test]
    fn test_security_fingerprint_override_round_trip() {
        let mut config = FreeCycleConfig::default();
        config.security.fingerprint_override = Some("custom-gpu-fingerprint".to_string());

        let serialized = toml::to_string_pretty(&config).unwrap();
        assert!(serialized.contains("fingerprint_override"));
        assert!(serialized.contains("custom-gpu-fingerprint"));

        let deserialized: FreeCycleConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(
            deserialized.security.fingerprint_override,
            Some("custom-gpu-fingerprint".to_string())
        );
        // Other security fields should remain None.
        assert_eq!(deserialized.security.keypair_path, None);
        assert_eq!(deserialized.security.cert_path, None);
        assert_eq!(deserialized.security.identity_uuid, None);
    }
}
