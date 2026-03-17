//! Windows auto-start management for FreeCycle.
//!
//! Handles three responsibilities:
//! 1. Registering FreeCycle to auto-start with Windows (registry Run key).
//! 2. Disabling Ollama's own auto-start (registry Run key and scheduled tasks).
//! 3. Enforcing Ollama's network binding to localhost via HKCU\Environment.

use anyhow::{Context, Result};
use tracing::{debug, info, warn};
use winreg::enums::*;
use winreg::RegKey;

/// Registry path for user auto-start programs.
const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Registry path for user-level persistent environment variables.
const USER_ENV_KEY_PATH: &str = "Environment";

/// The registry value name FreeCycle uses for its auto-start entry.
const FREECYCLE_REG_NAME: &str = "FreeCycle";

/// Known registry value names Ollama might use for auto-start.
const OLLAMA_REG_NAMES: &[&str] = &["Ollama", "ollama", "OllamaSetup"];

/// Registers FreeCycle to auto-start when the current user logs into Windows.
///
/// Writes the current executable path to
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\FreeCycle`.
///
/// # Errors
///
/// Returns an error if the registry key cannot be opened or the value cannot be set.
pub fn register_freecycle_autostart() -> Result<()> {
    let exe_path = std::env::current_exe()
        .context("Failed to get current executable path")?
        .to_string_lossy()
        .to_string();

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let run_key = hkcu
        .open_subkey_with_flags(RUN_KEY_PATH, KEY_SET_VALUE | KEY_READ)
        .context("Failed to open Run registry key")?;

    // Check if already registered with the current path
    if let Ok(existing) = run_key.get_value::<String, _>(FREECYCLE_REG_NAME) {
        if existing == exe_path {
            debug!("FreeCycle auto-start already registered with current path");
            return Ok(());
        }
    }

    run_key
        .set_value(FREECYCLE_REG_NAME, &exe_path)
        .context("Failed to set FreeCycle auto-start registry value")?;

    info!("Registered FreeCycle auto-start: {}", exe_path);
    Ok(())
}

/// Removes the FreeCycle auto-start registry entry.
///
/// # Errors
///
/// Returns an error if the registry key cannot be opened or the value cannot be deleted.
#[allow(dead_code)]
pub fn unregister_freecycle_autostart() -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let run_key = hkcu
        .open_subkey_with_flags(RUN_KEY_PATH, KEY_SET_VALUE)
        .context("Failed to open Run registry key")?;

    match run_key.delete_value(FREECYCLE_REG_NAME) {
        Ok(()) => {
            info!("Removed FreeCycle auto-start registry entry");
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            debug!("FreeCycle auto-start entry not found (already removed)");
            Ok(())
        }
        Err(e) => Err(e).context("Failed to delete FreeCycle auto-start registry value"),
    }
}

/// Disables Ollama's auto-start by removing its registry Run key entries
/// and disabling any related scheduled tasks.
///
/// Checks both `HKCU` and `HKLM` registry Run keys for known Ollama entries.
/// Also attempts to disable Ollama scheduled tasks via `schtasks`.
///
/// # Errors
///
/// Returns an error if registry operations fail. Scheduled task operations
/// failing is logged as a warning but does not cause a hard error.
pub fn disable_ollama_autostart() -> Result<()> {
    // Check HKCU Run key
    disable_ollama_registry_run(HKEY_CURRENT_USER, "HKCU")?;

    // Check HKLM Run key (may fail without admin, that is ok)
    if let Err(e) = disable_ollama_registry_run(HKEY_LOCAL_MACHINE, "HKLM") {
        debug!("Could not check HKLM Run key (may require admin): {}", e);
    }

    // Disable Ollama scheduled tasks
    disable_ollama_scheduled_tasks();

    Ok(())
}

/// Removes Ollama entries from a specific registry Run key.
///
/// # Arguments
///
/// * `hkey` - The registry root key (`HKEY_CURRENT_USER` or `HKEY_LOCAL_MACHINE`).
/// * `label` - Human-readable label for logging (e.g., "HKCU", "HKLM").
///
/// # Errors
///
/// Returns an error if the registry key cannot be opened.
fn disable_ollama_registry_run(hkey: winreg::HKEY, label: &str) -> Result<()> {
    let root = RegKey::predef(hkey);
    let run_key = match root.open_subkey_with_flags(RUN_KEY_PATH, KEY_SET_VALUE | KEY_READ) {
        Ok(key) => key,
        Err(e) => {
            debug!("Could not open {} Run key: {}", label, e);
            return Ok(());
        }
    };

    for name in OLLAMA_REG_NAMES {
        match run_key.delete_value(name) {
            Ok(()) => {
                info!("Disabled Ollama auto-start: {}\\{}", label, name);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!("Ollama auto-start entry '{}' not found in {}", name, label);
            }
            Err(e) => {
                warn!(
                    "Failed to remove Ollama auto-start '{}' from {}: {}",
                    name, label, e
                );
            }
        }
    }

    Ok(())
}

/// Enforces Ollama to bind only to the configured secure loopback address by
/// writing `OLLAMA_HOST` to the Windows user environment registry
/// (`HKCU\Environment`) and broadcasting `WM_SETTINGCHANGE` so all running
/// processes (including Ollama's own tray) pick up the change immediately.
///
/// This stops the kill/restart loop caused by Ollama's tray or autostart
/// relaunching Ollama bound to `0.0.0.0` after FreeCycle kills it.
///
/// # Arguments
///
/// * `host` - The loopback address to lock Ollama to (e.g. `"127.0.0.1"` or `"127.0.0.2"`).
/// * `port` - The Ollama port (builds `"<host>:<port>"`).
///
/// # Errors
///
/// Returns an error if the registry key cannot be opened or written.
pub fn enforce_ollama_localhost_binding(host: &str, port: u16) -> Result<()> {
    let host_value = format!("{}:{}", host, port);

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env_key = hkcu
        .open_subkey_with_flags(USER_ENV_KEY_PATH, KEY_SET_VALUE)
        .context("Failed to open HKCU\\Environment registry key")?;

    env_key
        .set_value("OLLAMA_HOST", &host_value)
        .context("Failed to set OLLAMA_HOST in user environment registry")?;

    info!("Set HKCU\\Environment\\OLLAMA_HOST = {}", host_value);

    broadcast_environment_change();

    Ok(())
}

/// Broadcasts `WM_SETTINGCHANGE` to all top-level windows so they refresh
/// their user environment block and pick up the new `OLLAMA_HOST` value.
///
/// This is a best-effort notification; failure is non-fatal.
fn broadcast_environment_change() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    let env_wide: Vec<u16> = OsStr::new("Environment")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut result: usize = 0;
    unsafe {
        SendMessageTimeoutW(
            0xFFFF as *mut std::ffi::c_void, // HWND_BROADCAST
            WM_SETTINGCHANGE,
            0,
            env_wide.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5000,
            &mut result,
        );
    }

    debug!("Broadcast WM_SETTINGCHANGE to propagate OLLAMA_HOST environment change");
}

/// Attempts to disable Ollama scheduled tasks using `schtasks`.
///
/// This is a best-effort operation. Failure is logged but not propagated
/// as scheduled tasks may not exist or may require admin privileges.
fn disable_ollama_scheduled_tasks() {
    let task_names = ["Ollama", "OllamaUpdate"];

    for task_name in &task_names {
        let result = std::process::Command::new("schtasks")
            .args(["/Change", "/TN", task_name, "/DISABLE"])
            .output();

        match result {
            Ok(output) if output.status.success() => {
                info!("Disabled Ollama scheduled task: {}", task_name);
            }
            Ok(_) => {
                debug!(
                    "Ollama scheduled task '{}' not found or already disabled",
                    task_name
                );
            }
            Err(e) => {
                debug!("Could not check scheduled task '{}': {}", task_name, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_path_is_valid() {
        assert!(RUN_KEY_PATH.contains("Run"));
    }

    #[test]
    fn test_ollama_reg_names_not_empty() {
        assert!(!OLLAMA_REG_NAMES.is_empty());
    }
}
