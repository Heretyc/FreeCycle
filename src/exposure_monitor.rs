//! Ollama exposure monitor for secure mode.
//!
//! Periodically inspects Windows listening sockets to detect if Ollama is exposed
//! on 0.0.0.0 (all interfaces) instead of 127.0.0.1 (localhost only) while secure mode
//! is active. If exposed, kills the Ollama process, logs a warning, and notifies the user
//! via the tray. The Ollama manager then restarts Ollama securely bound to 127.0.0.1.

use crate::notifications;
use crate::AppState;
use anyhow::{anyhow, Result};
use std::sync::Arc;
use std::time::Duration;
use sysinfo::System;
use tokio::sync::watch;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Represents a single listening socket entry from GetExtendedTcpTable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketEntry {
    /// Local IPv4 address (network byte order).
    pub local_addr: u32,
    /// Local port (host byte order).
    pub local_port: u16,
    /// Process ID owning this socket.
    pub pid: u32,
}

/// Enumerates all listening sockets on the system via GetExtendedTcpTable.
///
/// Uses the two-call allocation pattern: first call to get the required buffer size,
/// then allocate and call again to retrieve the table.
///
/// # Returns
///
/// A vector of `SocketEntry` structs representing all listening IPv4 sockets.
/// Returns an error if the Windows API calls fail.
pub fn enumerate_listening_sockets_raw() -> Result<Vec<SocketEntry>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    let mut pdw_size: u32 = 0;
    let mut sockets = Vec::new();

    // First call to determine required buffer size
    let result = unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut pdw_size,
            1, // bOrder: sorted
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };

    // ERROR_INSUFFICIENT_BUFFER is expected; we only use pdw_size
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    if result != ERROR_INSUFFICIENT_BUFFER && result != 0 {
        return Err(anyhow!(
            "GetExtendedTcpTable size query failed: code {}",
            result
        ));
    }

    // Allocate buffer and retry (with up to 3 retries if size changes between calls)
    for attempt in 0..3 {
        let mut buffer = vec![0u8; pdw_size as usize];

        let result = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr() as *mut std::ffi::c_void,
                &mut pdw_size,
                1,
                AF_INET as u32,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };

        if result == ERROR_INSUFFICIENT_BUFFER {
            if attempt < 2 {
                continue; // Retry with new size
            }
            return Err(anyhow!(
                "GetExtendedTcpTable: buffer too small after {} retries",
                attempt + 1
            ));
        }

        if result != 0 {
            return Err(anyhow!("GetExtendedTcpTable failed: code {}", result));
        }

        // Parse the MIB_TCPTABLE_OWNER_PID structure
        // Layout: DWORD dwNumEntries followed by dwNumEntries * MIB_TCPROW_OWNER_PID
        if buffer.len() < 4 {
            return Err(anyhow!("Buffer too small to read dwNumEntries"));
        }

        let dw_num_entries = u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);

        // MIB_TCPROW_OWNER_PID is 24 bytes: dwState, dwLocalAddr, dwLocalPort, dwRemoteAddr,
        // dwRemotePort, dwOwningPid (6 DWORDs x 4 bytes each)
        const ROW_SIZE: usize = 24;
        let expected_size = 4 + dw_num_entries as usize * ROW_SIZE;

        if buffer.len() < expected_size {
            return Err(anyhow!(
                "Buffer too small: expected {}, got {}",
                expected_size,
                buffer.len()
            ));
        }

        for i in 0..dw_num_entries as usize {
            let offset = 4 + i * ROW_SIZE;
            let row_data = &buffer[offset..offset + ROW_SIZE];

            // Parse MIB_TCPROW_OWNER_PID fields (all little-endian)
            let dw_local_addr = u32::from_le_bytes([row_data[4], row_data[5], row_data[6], row_data[7]]);
            let dw_local_port = u32::from_le_bytes([row_data[8], row_data[9], row_data[10], row_data[11]]);
            let pid = u32::from_le_bytes([row_data[20], row_data[21], row_data[22], row_data[23]]);

            // Convert port from network byte order (big-endian) to host byte order
            let local_port = ((dw_local_port as u16).swap_bytes()) as u16;

            sockets.push(SocketEntry {
                local_addr: dw_local_addr,
                local_port,
                pid,
            });
        }

        return Ok(sockets);
    }

    Err(anyhow!(
        "GetExtendedTcpTable: failed after maximum retries"
    ))
}

/// Pure logic function to find if Ollama is exposed on 0.0.0.0.
///
/// Iterates over the socket list and returns the PID of the first process
/// listening on 0.0.0.0 (represented as `local_addr == 0`) on the given port.
///
/// # Arguments
///
/// * `sockets` - Slice of socket entries from enumeration.
/// * `port` - Ollama port to check (host byte order).
///
/// # Returns
///
/// The PID of a process listening on 0.0.0.0:{port}, or None if not found or only bound to localhost.
pub fn find_exposed_ollama(sockets: &[SocketEntry], port: u16) -> Option<u32> {
    sockets
        .iter()
        .find(|entry| entry.local_addr == 0 && entry.local_port == port)
        .map(|entry| entry.pid)
}

/// Kills the given process by PID and updates the shared state.
///
/// Uses `sysinfo::System::process()` to locate and kill the process.
/// After killing, sets `ollama_running = false` in AppState so the Ollama
/// manager detects the change and restarts Ollama securely.
///
/// # Arguments
///
/// * `pid` - Process ID to kill.
/// * `state` - Shared application state.
fn kill_exposed_process(pid: u32, state: &mut AppState) {
    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, false);

    if let Some(proc) = sys.process(sysinfo::Pid::from_u32(pid)) {
        proc.kill();
        state.ollama_running = false;
        warn!("Killed exposed Ollama process (PID {})", pid);
    }
}

/// Fires a Windows balloon notification about the killed Ollama process.
///
/// Uses the HWND stored in AppState (set by the tray module). If no HWND
/// is available, silently skips the notification but logs a warning.
///
/// # Arguments
///
/// * `state` - Shared application state (contains notification_hwnd).
fn notify_exposure_kill(state: &AppState) {
    match state.notification_hwnd {
        Some(hwnd) => {
            notifications::show_balloon(
                hwnd as *mut std::ffi::c_void,
                "FreeCycle: Security Alert",
                "Ollama was exposed and has been restarted securely",
                notifications::BalloonKind::Warning,
            );
        }
        None => {
            warn!("Exposure monitor: notification_hwnd not available for balloon notification");
        }
    }
}

/// Runs the Ollama exposure monitor as a background task.
///
/// Periodically (every 60 seconds) checks if Ollama is listening on 0.0.0.0
/// while secure mode is active. If exposed, kills the process, logs a warning,
/// fires a notification, and lets the Ollama manager restart it securely.
///
/// Respects the `compatibility_mode` config flag and exits immediately if true.
/// Also gracefully responds to shutdown signals.
///
/// # Arguments
///
/// * `state` - Shared application state.
/// * `shutdown_rx` - Watch channel that signals application shutdown.
pub async fn run_exposure_monitor(
    state: Arc<RwLock<AppState>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    info!("Exposure monitor starting");

    loop {
        // Check compatibility_mode on each iteration
        {
            let app_state = state.read().await;
            if app_state.config.agent_server.compatibility_mode {
                info!("Exposure monitor: compatibility_mode is true, exiting");
                return;
            }
        }

        // Wait for next check or shutdown
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(60)) => {
                // Time to check
            }
            _ = shutdown_rx.changed() => {
                info!("Exposure monitor: shutdown signal received, exiting");
                break;
            }
        }

        // Read ollama_port from config
        let ollama_port = {
            let app_state = state.read().await;
            app_state.config.ollama.port
        };

        // Enumerate sockets and check for exposure
        match enumerate_listening_sockets_raw() {
            Ok(sockets) => {
                if let Some(exposed_pid) = find_exposed_ollama(&sockets, ollama_port) {
                    let mut app_state = state.write().await;
                    kill_exposed_process(exposed_pid, &mut app_state);
                    notify_exposure_kill(&app_state);
                }
            }
            Err(e) => {
                warn!("Exposure monitor: failed to enumerate sockets: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_exposed_ollama_exposed_on_all_interfaces() {
        let sockets = vec![SocketEntry {
            local_addr: 0,           // 0.0.0.0
            local_port: 11434,
            pid: 1234,
        }];
        assert_eq!(find_exposed_ollama(&sockets, 11434), Some(1234));
    }

    #[test]
    fn test_find_exposed_ollama_not_exposed() {
        let sockets = vec![SocketEntry {
            local_addr: 0x7f000001, // 127.0.0.1
            local_port: 11434,
            pid: 1234,
        }];
        assert_eq!(find_exposed_ollama(&sockets, 11434), None);
    }

    #[test]
    fn test_find_exposed_ollama_different_port() {
        let sockets = vec![SocketEntry {
            local_addr: 0,
            local_port: 8080,
            pid: 1234,
        }];
        assert_eq!(find_exposed_ollama(&sockets, 11434), None);
    }

    #[test]
    fn test_find_exposed_ollama_multiple_sockets_first_match() {
        let sockets = vec![
            SocketEntry {
                local_addr: 0x7f000001, // 127.0.0.1
                local_port: 11434,
                pid: 1000,
            },
            SocketEntry {
                local_addr: 0,           // 0.0.0.0
                local_port: 11434,
                pid: 2000,
            },
        ];
        assert_eq!(find_exposed_ollama(&sockets, 11434), Some(2000));
    }

    #[test]
    fn test_port_byte_order_conversion() {
        // Port 11434 in network byte order (big-endian) as a u16
        // 11434 = 0x2CAA in hex (network byte order: [0x2C, 0xAA])
        // When stored in a DWORD and read from buffer as u32 little-endian: 0x0000AA2C
        // (the port bytes are swapped in the little-endian storage: [0x2C, 0xAA] -> [0xAA, 0x2C])
        // After casting to u16 and swap_bytes(): 0x2CAA = 11434
        let dw_port = 0x0000AA2Cu32;
        let host_port = ((dw_port as u16).swap_bytes()) as u16;
        assert_eq!(host_port, 11434);
    }

    #[test]
    fn test_empty_socket_list() {
        let sockets: Vec<SocketEntry> = vec![];
        assert_eq!(find_exposed_ollama(&sockets, 11434), None);
    }
}
