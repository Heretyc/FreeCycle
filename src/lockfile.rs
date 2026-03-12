//! Process lock file management for FreeCycle.
//!
//! Prevents multiple instances of FreeCycle from running simultaneously.
//! Uses a lockfile with a timestamp at `%APPDATA%\FreeCycle\freecycle.lock`.
//! The lock automatically expires after 30 seconds to handle stale locks
//! from crashed instances.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
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
                    if let Ok(timestamp) = contents.trim().parse::<u64>() {
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
    #[allow(dead_code)]
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

/// Writes the current timestamp to the lockfile.
///
/// # Arguments
///
/// * `path` - Path to the lockfile.
///
/// # Errors
///
/// Returns an error if the file cannot be written.
fn write_lock(path: &PathBuf) -> Result<()> {
    let timestamp = current_timestamp().to_string();
    fs::write(path, &timestamp)
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
}
