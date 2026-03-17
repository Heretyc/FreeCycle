//! Process lock file management for FreeCycle.
//!
//! Prevents multiple instances of FreeCycle from running simultaneously.
//! Uses a lockfile with the owning PID and timestamp at
//! `%APPDATA%\FreeCycle\freecycle.lock`.
//! The lock automatically expires after 30 seconds to handle stale locks
//! from crashed or frozen instances.
//!
//! Lock file format (one value per line):
//! ```text
//! {pid}
//! {unix_timestamp_secs}
//! ```

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// Duration in seconds after which a stale lockfile is considered expired.
const LOCK_EXPIRY_SECONDS: u64 = 30;

/// Returns the path to the lockfile.
///
/// # Returns
///
/// Path to `%APPDATA%\FreeCycle\freecycle.lock`.
fn lock_path() -> PathBuf {
    crate::config::config_dir().join("freecycle.lock")
}

/// Represents an acquired process lock.
///
/// The lock is held for the lifetime of this struct. When dropped, the
/// lockfile is removed, allowing another instance to start.
///
/// # Example
///
/// ```no_run
/// use freecycle::lockfile::ProcessLock;
///
/// let lock = ProcessLock::acquire().unwrap();
/// if lock.is_none() {
///     println!("Another instance is running");
///     return;
/// }
/// let _lock = lock.unwrap(); // Lock held until _lock goes out of scope
/// ```
pub struct ProcessLock {
    /// Path to the lockfile.
    path: PathBuf,
}

impl ProcessLock {
    /// Attempts to acquire the process lock.
    ///
    /// If no lockfile exists, or the existing lockfile is expired (older than
    /// 30 seconds), creates a new lockfile and returns `Some(ProcessLock)`.
    /// If a valid (non-expired) lockfile exists, returns `None`.
    ///
    /// When taking over a stale lock that contains a PID, the old process is
    /// killed via `taskkill /F` to free any ports it may be holding.
    ///
    /// # Returns
    ///
    /// * `Ok(Some(ProcessLock))` - Lock acquired successfully.
    /// * `Ok(None)` - Another instance is running (lock not acquired).
    /// * `Err(_)` - An I/O error occurred.
    pub fn acquire() -> Result<Option<Self>> {
        let path = lock_path();

        // Ensure the directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create lock directory: {}", parent.display())
            })?;
        }

        // Check for existing lock
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(contents) => {
                    if let Some((old_pid, timestamp)) = parse_lock(&contents) {
                        let now = current_timestamp();
                        let age = now.saturating_sub(timestamp);
                        if age < LOCK_EXPIRY_SECONDS {
                            debug!(
                                "Lock is held (age: {}s, expires in {}s)",
                                age,
                                LOCK_EXPIRY_SECONDS - age
                            );
                            return Ok(None);
                        }
                        info!("Stale lock detected (age: {}s). Taking over.", age);
                        // Kill the old process to free its ports before we bind
                        if let Some(pid) = old_pid {
                            kill_old_process(pid);
                        }
                    } else {
                        warn!("Corrupted lockfile. Removing and re-acquiring.");
                    }
                }
                Err(e) => {
                    warn!("Could not read lockfile: {}. Removing and re-acquiring.", e);
                }
            }
            // Remove stale/corrupted lock
            let _ = fs::remove_file(&path);
        }

        // Write new lock
        write_lock(&path)?;

        Ok(Some(Self { path }))
    }

    /// Refreshes the lock timestamp to prevent expiry during long operations.
    ///
    /// Should be called periodically (more often than every 30 seconds) to
    /// keep the lock alive.
    ///
    /// # Errors
    ///
    /// Returns an error if the lockfile cannot be written.
    pub fn refresh(&self) -> Result<()> {
        write_lock(&self.path)
    }
}

impl Drop for ProcessLock {
    /// Removes the lockfile when the lock is dropped.
    fn drop(&mut self) {
        if let Err(e) = fs::remove_file(&self.path) {
            warn!("Failed to remove lockfile on drop: {}", e);
        } else {
            debug!("Process lock released");
        }
    }
}

/// Parses the lock file contents.
///
/// Supports two formats:
/// - New format: two lines — `{pid}\n{timestamp}`
/// - Legacy format: single line — `{timestamp}` (no PID available)
///
/// Returns `Some((Option<pid>, timestamp))` on success, `None` on parse error.
fn parse_lock(contents: &str) -> Option<(Option<u32>, u64)> {
    let mut lines = contents.trim().lines();
    let first = lines.next()?.trim();

    if let Some(second) = lines.next() {
        // New format: pid\ntimestamp
        let pid: u32 = first.parse().ok()?;
        let timestamp: u64 = second.trim().parse().ok()?;
        Some((Some(pid), timestamp))
    } else {
        // Legacy format: just a timestamp
        let timestamp: u64 = first.parse().ok()?;
        Some((None, timestamp))
    }
}

/// Kills a stale FreeCycle process by PID to release its held ports.
///
/// Uses sysinfo to verify the process exists, then `taskkill /F /PID` to
/// forcibly terminate it, then waits 2 seconds for the OS to release the port.
fn kill_old_process(pid: u32) {
    // Don't kill ourselves
    if pid == process::id() {
        return;
    }

    info!("Killing stale FreeCycle process (PID {}) to free port", pid);

    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, false);

    if sys.process(sysinfo::Pid::from_u32(pid)).is_none() {
        debug!("Old process PID {} no longer exists, skipping kill", pid);
        return;
    }

    match std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .output()
    {
        Ok(output) if output.status.success() => {
            info!("Killed stale FreeCycle process (PID {})", pid);
            // Wait briefly for the OS to release the port
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("taskkill failed for PID {}: {}", pid, stderr.trim());
        }
        Err(e) => {
            warn!("Failed to run taskkill for PID {}: {}", pid, e);
        }
    }
}

/// Writes the current PID and timestamp to the lockfile.
///
/// # Arguments
///
/// * `path` - Path to the lockfile.
///
/// # Errors
///
/// Returns an error if the file cannot be written.
fn write_lock(path: &PathBuf) -> Result<()> {
    let pid = process::id();
    let timestamp = current_timestamp();
    let content = format!("{}\n{}", pid, timestamp);
    fs::write(path, &content)
        .with_context(|| format!("Failed to write lockfile: {}", path.display()))?;
    Ok(())
}

/// Returns the current Unix timestamp in seconds.
///
/// # Returns
///
/// Seconds since the Unix epoch.
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_timestamp_is_reasonable() {
        let ts = current_timestamp();
        // Should be after 2024-01-01 (1704067200)
        assert!(ts > 1_704_067_200);
    }

    #[test]
    fn test_lock_expiry_constant() {
        assert_eq!(LOCK_EXPIRY_SECONDS, 30);
    }

    #[test]
    fn test_parse_lock_new_format() {
        let contents = "12345\n1704067200\n";
        let result = parse_lock(contents);
        assert_eq!(result, Some((Some(12345), 1704067200)));
    }

    #[test]
    fn test_parse_lock_legacy_format() {
        let contents = "1704067200\n";
        let result = parse_lock(contents);
        assert_eq!(result, Some((None, 1704067200)));
    }

    #[test]
    fn test_parse_lock_corrupted() {
        assert_eq!(parse_lock("not a number"), None);
        assert_eq!(parse_lock(""), None);
        assert_eq!(parse_lock("abc\n123"), None);
    }
}
