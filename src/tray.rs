//! System tray icon management for FreeCycle.
//!
//! Displays a colored icon in the Windows system tray that reflects the
//! current state: green (available), red (blocked/cooldown), blue (agent task),
//! yellow (downloading), grey (error). Updates the tooltip every 2 seconds
//! with VRAM usage, Ollama status, IP/port, and active task info.

use crate::state::FreeCycleStatus;
use crate::AppState;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use tokio::sync::{watch, RwLock};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};
use tracing::{debug, info};

/// RGBA color values for each tray icon state.
const COLOR_GREEN: [u8; 4] = [0x2E, 0xCC, 0x40, 0xFF];   // Available
const COLOR_RED: [u8; 4] = [0xFF, 0x41, 0x36, 0xFF];      // Blocked/Cooldown
const COLOR_BLUE: [u8; 4] = [0x00, 0x74, 0xD9, 0xFF];     // Agent Task Active
const COLOR_YELLOW: [u8; 4] = [0xFF, 0xDC, 0x00, 0xFF];   // Downloading
const COLOR_GREY: [u8; 4] = [0xAA, 0xAA, 0xAA, 0xFF];     // Error/Initializing

/// Size of the generated tray icon in pixels.
const ICON_SIZE: u32 = 32;

/// Generates a solid colored circle icon as RGBA bytes.
///
/// Creates a `ICON_SIZE x ICON_SIZE` image with a filled circle in the
/// specified color on a transparent background.
///
/// # Arguments
///
/// * `color` - RGBA color for the circle.
///
/// # Returns
///
/// A `tray_icon::Icon` with the generated image.
fn make_icon(color: [u8; 4]) -> Icon {
    let mut rgba = vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize];
    let center = ICON_SIZE as f32 / 2.0;
    let radius = center - 2.0;

    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            if dx * dx + dy * dy <= radius * radius {
                let idx = ((y * ICON_SIZE + x) * 4) as usize;
                rgba[idx] = color[0];
                rgba[idx + 1] = color[1];
                rgba[idx + 2] = color[2];
                rgba[idx + 3] = color[3];
            }
        }
    }

    Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE).expect("Failed to create icon")
}

/// Returns the icon color for the given status.
///
/// # Arguments
///
/// * `status` - The current FreeCycle status.
/// * `models_downloading` - Whether models are currently being downloaded.
///
/// # Returns
///
/// RGBA color array for the icon.
fn status_color(status: &FreeCycleStatus, models_downloading: bool) -> [u8; 4] {
    if models_downloading
        && matches!(
            status,
            FreeCycleStatus::Available | FreeCycleStatus::Downloading
        )
    {
        return COLOR_YELLOW;
    }

    match status {
        FreeCycleStatus::Initializing => COLOR_GREY,
        FreeCycleStatus::Available => COLOR_GREEN,
        FreeCycleStatus::Blocked => COLOR_RED,
        FreeCycleStatus::Cooldown { .. } => COLOR_RED,
        FreeCycleStatus::AgentTaskActive => COLOR_BLUE,
        FreeCycleStatus::Downloading => COLOR_YELLOW,
        FreeCycleStatus::Error(_) => COLOR_GREY,
    }
}

/// Builds the tooltip string from the current application state.
///
/// Includes: status, VRAM usage, Ollama port, local IP, cooldown timer,
/// blocking processes, agent task info, and model status.
///
/// # Arguments
///
/// * `state` - Reference to the current application state.
///
/// # Returns
///
/// A formatted tooltip string (max 128 chars for Windows, truncated if needed).
fn build_tooltip(state: &AppState) -> String {
    let mut lines: Vec<String> = Vec::new();

    // Status line
    lines.push(format!("FreeCycle: {}", state.status.label()));

    // VRAM usage
    if state.vram_total_bytes > 0 {
        let used_mb = state.vram_used_bytes / (1024 * 1024);
        let total_mb = state.vram_total_bytes / (1024 * 1024);
        let pct = state.vram_used_bytes * 100 / state.vram_total_bytes;
        lines.push(format!("VRAM: {} / {} MB ({}%)", used_mb, total_mb, pct));
    }

    // Ollama status and network info
    if state.ollama_running {
        lines.push(format!(
            "Ollama: {}:{} (running)",
            state.local_ip, state.config.ollama.port
        ));
    } else {
        lines.push("Ollama: stopped".to_string());
    }

    // Cooldown timer
    if let FreeCycleStatus::Cooldown { expires_at } = &state.status {
        let remaining = expires_at
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO);
        lines.push(format!("Cooldown: {}s remaining", remaining.as_secs()));
    }

    // Blocking processes
    if !state.blocking_processes.is_empty() {
        lines.push(format!("Blocked by: {}", state.blocking_processes.join(", ")));
    }

    // Agent task info
    if let Some(ref task) = state.agent_task {
        lines.push(format!("Task: {} (from {})", task.description, task.source_ip));
    }

    // Model status
    for status in &state.model_status {
        lines.push(status.clone());
    }

    // Agent server port
    lines.push(format!("Agent API: port {}", state.config.agent_server.port));

    let tooltip = lines.join("\n");

    // Windows tooltip max is 128 chars; truncate if needed
    if tooltip.len() > 127 {
        format!("{}...", &tooltip[..124])
    } else {
        tooltip
    }
}

/// Runs the system tray icon and Windows message loop.
///
/// This function blocks the calling thread (must be the main thread on Windows)
/// and runs the Win32 message pump. It updates the tray icon and tooltip
/// on the configured interval (default 2 seconds).
///
/// # Arguments
///
/// * `state` - Shared application state.
/// * `shutdown_tx` - Watch channel sender to signal shutdown to all subsystems.
/// * `runtime` - Reference to the Tokio runtime for blocking on async state reads.
///
/// # Errors
///
/// Returns an error if the tray icon cannot be created.
pub fn run_tray(
    state: Arc<RwLock<AppState>>,
    shutdown_tx: watch::Sender<bool>,
    runtime: &Runtime,
) -> Result<()> {
    // Build context menu
    let menu = Menu::new();
    let item_status = MenuItem::new("Status: Initializing", false, None);
    let item_force_enable = MenuItem::new("Force Enable Ollama", true, None);
    let item_force_disable = MenuItem::new("Force Disable Ollama", true, None);
    let item_open_logs = MenuItem::new("Open Logs", true, None);
    let item_open_config = MenuItem::new("Open Config", true, None);
    let item_quit = MenuItem::new("Exit FreeCycle", true, None);

    menu.append(&item_status).ok();
    menu.append(&PredefinedMenuItem::separator()).ok();
    menu.append(&item_force_enable).ok();
    menu.append(&item_force_disable).ok();
    menu.append(&PredefinedMenuItem::separator()).ok();
    menu.append(&item_open_logs).ok();
    menu.append(&item_open_config).ok();
    menu.append(&PredefinedMenuItem::separator()).ok();
    menu.append(&item_quit).ok();

    // Create tray icon
    let initial_icon = make_icon(COLOR_GREY);
    let tray = TrayIconBuilder::new()
        .with_icon(initial_icon)
        .with_tooltip("FreeCycle: Initializing...")
        .with_menu(Box::new(menu))
        .build()
        .context("Failed to create tray icon")?;

    info!("System tray icon created");

    let mut last_color = COLOR_GREY;
    let mut last_update = Instant::now();

    // Win32 message loop
    unsafe {
        let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = std::mem::zeroed();

        loop {
            // Process menu events
            if let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id() == item_quit.id() {
                    info!("Exit requested via tray menu");
                    let _ = shutdown_tx.send(true);
                    break;
                } else if event.id() == item_open_logs.id() {
                    let log_path = dirs::home_dir()
                        .unwrap_or_default()
                        .join("freecycle-verbose.log");
                    let _ = std::process::Command::new("notepad")
                        .arg(log_path)
                        .spawn();
                } else if event.id() == item_open_config.id() {
                    let config_path = crate::config::config_path();
                    let _ = std::process::Command::new("notepad")
                        .arg(config_path)
                        .spawn();
                }
            }

            // Update tray icon and tooltip periodically
            let update_interval = runtime.block_on(async {
                let s = state.read().await;
                Duration::from_millis(s.config.general.tray_update_interval_ms)
            });

            if last_update.elapsed() >= update_interval {
                let (new_color, tooltip) = runtime.block_on(async {
                    let s = state.read().await;
                    let color = status_color(&s.status, s.models_downloading);
                    let tooltip = build_tooltip(&s);
                    (color, tooltip)
                });

                // Only update icon if color changed
                if new_color != last_color {
                    let icon = make_icon(new_color);
                    tray.set_icon(Some(icon)).ok();
                    last_color = new_color;
                    debug!("Tray icon color updated");
                }

                tray.set_tooltip(Some(&tooltip)).ok();
                last_update = Instant::now();
            }

            // Pump Win32 messages (with a short timeout to stay responsive)
            let result = windows_sys::Win32::UI::WindowsAndMessaging::PeekMessageW(
                &mut msg,
                std::ptr::null_mut(),
                0,
                0,
                windows_sys::Win32::UI::WindowsAndMessaging::PM_REMOVE,
            );

            if result != 0 {
                windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
                windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
            } else {
                // No messages, sleep briefly to avoid busy-waiting
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_icon_does_not_panic() {
        let _icon = make_icon(COLOR_GREEN);
        let _icon = make_icon(COLOR_RED);
        let _icon = make_icon(COLOR_BLUE);
        let _icon = make_icon(COLOR_YELLOW);
        let _icon = make_icon(COLOR_GREY);
    }

    #[test]
    fn test_status_color_mapping() {
        assert_eq!(status_color(&FreeCycleStatus::Available, false), COLOR_GREEN);
        assert_eq!(status_color(&FreeCycleStatus::Blocked, false), COLOR_RED);
        assert_eq!(
            status_color(&FreeCycleStatus::AgentTaskActive, false),
            COLOR_BLUE
        );
        assert_eq!(
            status_color(&FreeCycleStatus::Error("test".into()), false),
            COLOR_GREY
        );
    }

    #[test]
    fn test_status_color_downloading_overlay() {
        assert_eq!(status_color(&FreeCycleStatus::Available, true), COLOR_YELLOW);
    }

    #[test]
    fn test_tooltip_truncation() {
        let config = crate::config::FreeCycleConfig::default();
        let mut state = AppState::new(config);
        state.status = FreeCycleStatus::Available;
        state.ollama_running = true;
        state.vram_used_bytes = 2 * 1024 * 1024 * 1024;
        state.vram_total_bytes = 8 * 1024u64 * 1024 * 1024;

        let tooltip = build_tooltip(&state);
        assert!(tooltip.len() <= 127 || tooltip.ends_with("..."));
    }
}
