//! GPU monitoring subsystem for FreeCycle.
//!
//! Runs on a 5-second interval, checking for blacklisted processes and
//! non-whitelisted VRAM usage above the configured threshold. Updates
//! the shared application state to drive Ollama lifecycle decisions.

use crate::state::FreeCycleStatus;
use crate::AppState;
use nvml_wrapper::Nvml;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use sysinfo::System;
use tokio::sync::{watch, RwLock};
use tracing::{debug, error, info, warn};

/// Runs the GPU monitoring loop on a 5-second interval.
///
/// Checks for blacklisted processes and VRAM usage from non-whitelisted processes.
/// Updates `AppState.status` to reflect whether the GPU is available, blocked,
/// or in cooldown.
///
/// # Arguments
///
/// * `state` - Shared application state.
/// * `shutdown_rx` - Watch channel that signals when the application is shutting down.
pub async fn run_gpu_monitor(
    state: Arc<RwLock<AppState>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let nvml = match Nvml::init() {
        Ok(nvml) => nvml,
        Err(e) => {
            error!("Failed to initialize NVML: {}. GPU monitoring disabled.", e);
            let mut s = state.write().await;
            s.status = FreeCycleStatus::Error(format!("NVML init failed: {}", e));
            return;
        }
    };

    let device = match nvml.device_by_index(0) {
        Ok(d) => d,
        Err(e) => {
            error!("Failed to get GPU device: {}. GPU monitoring disabled.", e);
            let mut s = state.write().await;
            s.status = FreeCycleStatus::Error(format!("GPU device error: {}", e));
            return;
        }
    };

    info!("GPU monitoring started");

    let mut sys = System::new();

    loop {
        let interval = {
            let s = state.read().await;
            Duration::from_millis(s.config.general.gpu_check_interval_ms)
        };

        tokio::select! {
            _ = tokio::time::sleep(interval) => {},
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("GPU monitor shutting down");
                    return;
                }
            }
        }

        // Refresh process list
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

        // Build PID to process name map
        let pid_name_map: HashMap<u32, String> = sys
            .processes()
            .iter()
            .map(|(pid, proc_)| (pid.as_u32(), proc_.name().to_string_lossy().to_string()))
            .collect();

        // Check for blacklisted processes
        let blacklisted_detected = {
            let s = state.read().await;
            find_blacklisted_processes(&pid_name_map, &s.config.blacklisted_processes.list)
        };

        // Query VRAM usage
        let mem_info = match device.memory_info() {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to query VRAM: {}", e);
                continue;
            }
        };

        // Get per-process GPU usage (both graphics and compute)
        let mut gpu_processes = device.running_graphics_processes().unwrap_or_default();
        gpu_processes.extend(device.running_compute_processes().unwrap_or_default());

        // Calculate non-whitelisted VRAM usage
        let non_whitelisted_vram = {
            let s = state.read().await;
            calculate_non_whitelisted_vram(
                &gpu_processes,
                &pid_name_map,
                &s.config.whitelisted_processes.list,
            )
        };

        let vram_threshold_bytes = {
            let s = state.read().await;
            mem_info.total * s.config.general.vram_threshold_percent / 100
        };

        let high_vram_usage = non_whitelisted_vram > vram_threshold_bytes;

        // Update shared state
        let mut s = state.write().await;
        s.vram_used_bytes = mem_info.used;
        s.vram_total_bytes = mem_info.total;
        s.blocking_processes = blacklisted_detected.clone();

        // Track agent task idle state
        let vram_idle_threshold = s.config.general.vram_idle_mb * 1024 * 1024;
        if mem_info.used < vram_idle_threshold {
            if s.vram_idle_since.is_none() {
                s.vram_idle_since = Some(Instant::now());
            }
        } else {
            s.vram_idle_since = None;
        }

        // State machine transitions
        let now = Instant::now();

        if !blacklisted_detected.is_empty() {
            // Blacklisted process running: block immediately
            s.last_blacklist_seen = Some(now);
            if s.status != FreeCycleStatus::Blocked {
                info!(
                    "Blacklisted process detected: {:?}. Blocking GPU access.",
                    blacklisted_detected
                );
            }
            s.status = FreeCycleStatus::Blocked;
            s.agent_task = None;
        } else if let Some(last_seen) = s.last_blacklist_seen {
            let cooldown = Duration::from_secs(s.config.general.cooldown_seconds);
            let elapsed = now.duration_since(last_seen);
            if elapsed < cooldown {
                let expires_at = last_seen + cooldown;
                debug!(
                    "Cooldown active: {} seconds remaining",
                    (cooldown - elapsed).as_secs()
                );
                s.status = FreeCycleStatus::Cooldown { expires_at };
            } else {
                // Cooldown expired
                s.last_blacklist_seen = None;
                transition_to_available_or_agent(&mut s, high_vram_usage);
            }
        } else if high_vram_usage {
            // Non-whitelisted process using too much VRAM
            debug!(
                "High VRAM usage from non-whitelisted processes: {} MB / {} MB threshold",
                non_whitelisted_vram / (1024 * 1024),
                vram_threshold_bytes / (1024 * 1024)
            );
            s.status = FreeCycleStatus::Blocked;
        } else {
            transition_to_available_or_agent(&mut s, high_vram_usage);
        }

        // Check agent task timeout
        check_agent_task_timeout(&mut s);
    }
}

/// Transitions the state to Available or AgentTaskActive depending on whether
/// an agent task is currently tracked.
///
/// # Arguments
///
/// * `s` - Mutable reference to the app state.
/// * `high_vram_usage` - Whether non-whitelisted VRAM usage is above threshold.
fn transition_to_available_or_agent(
    s: &mut AppState,
    _high_vram_usage: bool,
) {
    if s.agent_task.is_some() {
        s.status = FreeCycleStatus::AgentTaskActive;
    } else if s.models_downloading {
        s.status = FreeCycleStatus::Downloading;
    } else {
        s.status = FreeCycleStatus::Available;
    }
}

/// Checks if the agent task should be cleared due to timeout or idle VRAM.
///
/// Rules:
/// - If VRAM is below 300MB for more than 3 minutes, revert to green (clear task)
///   BUT if VRAM goes back up, re-assume the same task (icon goes blue again).
/// - If no VRAM usage for more than 1 hour, clear the task entirely.
///
/// # Arguments
///
/// * `s` - Mutable reference to the app state.
fn check_agent_task_timeout(s: &mut AppState) {
    if let Some(ref task) = s.agent_task {
        let idle_timeout = Duration::from_secs(s.config.general.vram_idle_timeout_minutes * 60);
        let task_timeout = Duration::from_secs(s.config.general.task_timeout_hours * 3600);

        // Check 1-hour absolute timeout
        if task.started_at.elapsed() > task_timeout {
            info!(
                "Agent task '{}' timed out after {} hours. Clearing.",
                task.task_id, s.config.general.task_timeout_hours
            );
            s.agent_task = None;
            return;
        }

        // Check 3-minute idle timeout (VRAM below 300MB)
        if let Some(idle_since) = s.vram_idle_since {
            if idle_since.elapsed() > idle_timeout {
                debug!(
                    "VRAM idle for {} minutes. Reverting from blue to green (task still tracked).",
                    idle_since.elapsed().as_secs() / 60
                );
                // Note: we do NOT clear agent_task here. We just show green.
                // If VRAM goes back up, we show blue again with the same task.
                // The task is only fully cleared by the 1-hour timeout or a stop signal.
            }
        }
    }
}

/// Finds blacklisted processes in the current process list.
///
/// Comparison is case-insensitive to handle variations in process naming.
///
/// # Arguments
///
/// * `pid_name_map` - Map of PID to process name from sysinfo.
/// * `blacklist` - List of blacklisted process names.
///
/// # Returns
///
/// Names of currently running blacklisted processes.
fn find_blacklisted_processes(
    pid_name_map: &HashMap<u32, String>,
    blacklist: &[String],
) -> Vec<String> {
    let mut found = Vec::new();
    let blacklist_lower: Vec<String> = blacklist.iter().map(|s| s.to_lowercase()).collect();

    for name in pid_name_map.values() {
        let name_lower = name.to_lowercase();
        if blacklist_lower.contains(&name_lower) && !found.contains(name) {
            found.push(name.clone());
        }
    }
    found
}

/// Calculates total VRAM usage from non-whitelisted processes.
///
/// # Arguments
///
/// * `gpu_processes` - List of GPU processes from NVML.
/// * `pid_name_map` - Map of PID to process name from sysinfo.
/// * `whitelist` - List of whitelisted process names.
///
/// # Returns
///
/// Total VRAM in bytes used by non-whitelisted processes.
fn calculate_non_whitelisted_vram(
    gpu_processes: &[nvml_wrapper::struct_wrappers::device::ProcessInfo],
    pid_name_map: &HashMap<u32, String>,
    whitelist: &[String],
) -> u64 {
    let whitelist_lower: Vec<String> = whitelist.iter().map(|s| s.to_lowercase()).collect();
    let mut total: u64 = 0;

    for proc in gpu_processes {
        let name = pid_name_map
            .get(&proc.pid)
            .cloned()
            .unwrap_or_default()
            .to_lowercase();

        let is_whitelisted = whitelist_lower.iter().any(|w| name.contains(w));
        if !is_whitelisted {
            if let nvml_wrapper::enums::device::UsedGpuMemory::Used(bytes) = proc.used_gpu_memory {
                total += bytes;
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_blacklisted_processes_empty() {
        let map = HashMap::new();
        let blacklist = vec!["VRChat.exe".to_string()];
        assert!(find_blacklisted_processes(&map, &blacklist).is_empty());
    }

    #[test]
    fn test_find_blacklisted_processes_case_insensitive() {
        let mut map = HashMap::new();
        map.insert(1, "vrchat.exe".to_string());
        map.insert(2, "explorer.exe".to_string());
        let blacklist = vec!["VRChat.exe".to_string()];
        let found = find_blacklisted_processes(&map, &blacklist);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], "vrchat.exe");
    }

    #[test]
    fn test_find_blacklisted_no_duplicates() {
        let mut map = HashMap::new();
        map.insert(1, "VRChat.exe".to_string());
        map.insert(2, "VRChat.exe".to_string());
        let blacklist = vec!["VRChat.exe".to_string()];
        let found = find_blacklisted_processes(&map, &blacklist);
        assert_eq!(found.len(), 1);
    }
}
