//! System tray icon management for FreeCycle.
//!
//! Displays a colored icon in the Windows system tray that reflects the
//! current state: green (available), red (blocked/cooldown), blue (agent task),
//! yellow (downloading), grey (error). Updates the tooltip every 2 seconds
//! with VRAM usage, Ollama status, IP/port, and active task info.

use crate::lockfile::ProcessLock;
use crate::state::{FreeCycleStatus, ManualOverride};
use crate::{AppState, REMOTE_MODEL_INSTALL_UNLOCK_DURATION};
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use tokio::sync::{watch, RwLock};
use tracing::{debug, info, warn};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Power::{
    RegisterSuspendResumeNotification, UnregisterSuspendResumeNotification, HPOWERNOTIFY,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, RegisterClassW,
    SetWindowLongPtrW, UnregisterClassW, CREATESTRUCTW, DEVICE_NOTIFY_WINDOW_HANDLE, GWLP_USERDATA,
    HMENU, HWND_MESSAGE, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
    WINDOW_EX_STYLE, WM_DESTROY, WM_NCCREATE, WM_POWERBROADCAST, WNDCLASSW, WS_OVERLAPPED,
};

/// RGBA color values for each tray icon state.
const COLOR_GREEN: [u8; 4] = [0x2E, 0xCC, 0x40, 0xFF]; // Available
const COLOR_RED: [u8; 4] = [0xFF, 0x41, 0x36, 0xFF]; // Blocked/Cooldown
const COLOR_BLUE: [u8; 4] = [0x00, 0x74, 0xD9, 0xFF]; // Agent Task Active
const COLOR_YELLOW: [u8; 4] = [0xFF, 0xDC, 0x00, 0xFF]; // Downloading
const COLOR_GREY: [u8; 4] = [0xAA, 0xAA, 0xAA, 0xFF]; // Error/Initializing

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
        FreeCycleStatus::WakeDelay { .. } => COLOR_RED,
        FreeCycleStatus::AgentTaskActive => COLOR_BLUE,
        FreeCycleStatus::Downloading => COLOR_YELLOW,
        FreeCycleStatus::Error(_) => COLOR_GREY,
    }
}

fn menu_status_label(status: &FreeCycleStatus, manual_override: Option<ManualOverride>) -> String {
    match manual_override {
        Some(override_mode) => format!("Status: {} ({})", status.label(), override_mode.label()),
        None => format!("Status: {}", status.label()),
    }
}

fn force_enable_item_enabled(manual_override: Option<ManualOverride>) -> bool {
    manual_override != Some(ManualOverride::ForceEnable)
}

fn force_disable_item_enabled(manual_override: Option<ManualOverride>) -> bool {
    manual_override != Some(ManualOverride::ForceDisable)
}

fn format_remaining_duration(duration: Duration) -> String {
    let seconds = duration.as_secs().max(1);
    if seconds >= 3600 {
        let hours = seconds.div_ceil(3600);
        format!("{}h left", hours)
    } else if seconds >= 120 {
        let minutes = seconds.div_ceil(60);
        format!("{}m left", minutes)
    } else {
        format!("{}s left", seconds)
    }
}

fn remote_model_install_menu_label(state: &AppState, now: Instant) -> String {
    match state.remote_model_install_unlock_remaining(now) {
        Some(remaining) => format!(
            "Disable Remote Model Installs ({})",
            format_remaining_duration(remaining)
        ),
        None => format!(
            "Enable Remote Model Installs ({} Hour)",
            REMOTE_MODEL_INSTALL_UNLOCK_DURATION.as_secs() / 3600
        ),
    }
}

struct PowerEventContext {
    state: Arc<RwLock<AppState>>,
    runtime: *const Runtime,
    suspend_seen_since_resume: AtomicBool,
}

fn wake_delay_replaces_visible_status(status: &FreeCycleStatus) -> bool {
    !matches!(
        status,
        FreeCycleStatus::Blocked | FreeCycleStatus::Cooldown { .. } | FreeCycleStatus::Error(_)
    )
}

fn apply_resume_wake_delay(
    state: &mut AppState,
    now: Instant,
    saw_suspend_since_last_resume: bool,
) -> bool {
    if !saw_suspend_since_last_resume
        && matches!(state.wake_block_until, Some(existing_deadline) if existing_deadline > now)
    {
        return false;
    }

    let wake_delay = Duration::from_secs(state.config.general.wake_delay_seconds);
    let wake_block_until = now + wake_delay;
    state.wake_block_until = Some(wake_block_until);

    if state.manual_override == Some(ManualOverride::ForceEnable) {
        state.manual_override = None;
    }

    if wake_delay_replaces_visible_status(&state.status) {
        state.status = FreeCycleStatus::WakeDelay {
            expires_at: wake_block_until,
        };
    }

    true
}

unsafe extern "system" fn power_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let create_struct = lparam as *const CREATESTRUCTW;
        if !create_struct.is_null() {
            let ctx = unsafe { (*create_struct).lpCreateParams } as *mut PowerEventContext;
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx as isize);
            }
        }
    }

    let ctx_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut PowerEventContext;

    if !ctx_ptr.is_null() && msg == WM_POWERBROADCAST {
        let ctx = unsafe { &*ctx_ptr };
        match wparam as u32 {
            PBT_APMSUSPEND => {
                ctx.suspend_seen_since_resume.store(true, Ordering::Relaxed);
                info!("Received system suspend notification");
            }
            PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => {
                let runtime = unsafe { &*ctx.runtime };
                let saw_suspend_since_last_resume =
                    ctx.suspend_seen_since_resume.swap(false, Ordering::Relaxed);
                runtime.block_on(async {
                    let mut state = ctx.state.write().await;
                    let cleared_force_enable =
                        state.manual_override == Some(ManualOverride::ForceEnable);
                    let wake_delay_seconds = state.config.general.wake_delay_seconds;

                    if apply_resume_wake_delay(
                        &mut state,
                        Instant::now(),
                        saw_suspend_since_last_resume,
                    ) {
                        if cleared_force_enable {
                            info!(
                                "Received system resume notification. Applying {}s wake delay and clearing force enable override.",
                                wake_delay_seconds
                            );
                        } else {
                            info!(
                                "Received system resume notification. Applying {}s wake delay.",
                                wake_delay_seconds
                            );
                        }
                    } else {
                        debug!(
                            "Ignoring duplicate system resume notification while wake delay is already active."
                        );
                    }
                });
            }
            _ => {}
        }

        return 1;
    }

    if msg == WM_DESTROY {
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        }
    }

    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn to_wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn destroy_power_window(
    power_window: HWND,
    class_name: *const u16,
    power_notification_handle: HPOWERNOTIFY,
    power_context_ptr: *mut PowerEventContext,
) {
    if power_notification_handle != 0
        && unsafe { UnregisterSuspendResumeNotification(power_notification_handle) } == 0
    {
        warn!("Failed to unregister suspend or resume notifications");
    }

    if !power_window.is_null() {
        unsafe {
            DestroyWindow(power_window);
        }
    }

    if !class_name.is_null() {
        unsafe {
            UnregisterClassW(class_name, std::ptr::null_mut());
        }
    }

    if !power_context_ptr.is_null() {
        unsafe {
            drop(Box::from_raw(power_context_ptr));
        }
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
    let now = Instant::now();

    // Status line
    let status_line = match state.manual_override {
        Some(ManualOverride::ForceEnable) => {
            format!("FreeCycle: Forced Available ({})", state.status.label())
        }
        Some(ManualOverride::ForceDisable) => {
            format!("FreeCycle: Forced Stop ({})", state.status.label())
        }
        None => format!("FreeCycle: {}", state.status.label()),
    };
    lines.push(status_line);

    if let Some(remaining) = state.remote_model_install_unlock_remaining(now) {
        lines.push(format!(
            "Remote installs: {}",
            format_remaining_duration(remaining)
        ));
    }

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

    if let FreeCycleStatus::WakeDelay { expires_at } = &state.status {
        let remaining = expires_at
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO);
        lines.push(format!("Wake delay: {}s remaining", remaining.as_secs()));
    }

    // Blocking processes
    if !state.blocking_processes.is_empty() {
        lines.push(format!(
            "Blocked by: {}",
            state.blocking_processes.join(", ")
        ));
    }

    // Agent task info
    if let Some(ref task) = state.agent_task {
        lines.push(format!(
            "Task: {} (from {})",
            task.description, task.source_ip
        ));
    }

    if let Some(override_mode) = state.manual_override {
        lines.push(format!("Override: {}", override_mode.label()));
    }

    // Active model progress should outrank less important metadata so percentages remain visible.
    for status in &state.model_status {
        if status.starts_with("Downloading ")
            || status.starts_with("Updating ")
            || status.starts_with("Failed: ")
        {
            lines.push(status.clone());
        }
    }

    // Agent server port
    lines.push(format!(
        "Agent API: port {}",
        state.config.agent_server.port
    ));

    let mut tooltip_lines: Vec<String> = Vec::new();
    let mut total_len = 0usize;

    for line in lines {
        let separator_len = if tooltip_lines.is_empty() { 0 } else { 1 };
        if total_len + separator_len + line.len() > 127 {
            break;
        }

        total_len += separator_len + line.len();
        tooltip_lines.push(line);
    }

    if tooltip_lines.is_empty() {
        return "FreeCycle".to_string();
    }

    tooltip_lines.join("\n")
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
    let class_name = to_wide_null("FreeCyclePowerEvents");
    let window_name = to_wide_null("FreeCycle Power Event Window");
    let power_context = Box::new(PowerEventContext {
        state: Arc::clone(&state),
        runtime: runtime as *const Runtime,
        suspend_seen_since_resume: AtomicBool::new(false),
    });
    let power_context_ptr = Box::into_raw(power_context);

    let power_window = unsafe {
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(power_window_proc),
            lpszClassName: class_name.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&window_class);
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name.as_ptr(),
            window_name.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            0 as HMENU,
            std::ptr::null_mut(),
            power_context_ptr.cast(),
        )
    };

    if power_window.is_null() {
        unsafe {
            destroy_power_window(power_window, class_name.as_ptr(), 0, power_context_ptr);
        }
        anyhow::bail!("Failed to create hidden power event window");
    }

    let power_notification_handle =
        unsafe { RegisterSuspendResumeNotification(power_window, DEVICE_NOTIFY_WINDOW_HANDLE) };

    if power_notification_handle == 0 {
        unsafe {
            destroy_power_window(
                power_window,
                class_name.as_ptr(),
                power_notification_handle,
                power_context_ptr,
            );
        }
        anyhow::bail!("Failed to register suspend or resume notifications");
    }

    info!("Registered hidden tray window for suspend or resume notifications");

    // Build context menu
    let menu = Menu::new();
    let item_status = MenuItem::new("Status: Initializing", false, None);
    let item_force_enable = MenuItem::new("Force Enable Ollama", true, None);
    let item_force_disable = MenuItem::new("Force Disable Ollama", true, None);
    let item_remote_model_installs =
        MenuItem::new("Enable Remote Model Installs (1 Hour)", true, None);
    let item_open_logs = MenuItem::new("Open Logs", true, None);
    let item_open_config = MenuItem::new("Open Config", true, None);
    let item_quit = MenuItem::new("Exit FreeCycle", true, None);

    menu.append(&item_status).ok();
    menu.append(&PredefinedMenuItem::separator()).ok();
    menu.append(&item_force_enable).ok();
    menu.append(&item_force_disable).ok();
    menu.append(&item_remote_model_installs).ok();
    menu.append(&PredefinedMenuItem::separator()).ok();
    menu.append(&item_open_logs).ok();
    menu.append(&item_open_config).ok();
    menu.append(&PredefinedMenuItem::separator()).ok();
    menu.append(&item_quit).ok();

    // Create tray icon
    let initial_icon = make_icon(COLOR_GREY);
    let tray = match TrayIconBuilder::new()
        .with_icon(initial_icon)
        .with_tooltip("FreeCycle: Initializing...")
        .with_menu(Box::new(menu))
        .build()
    {
        Ok(tray) => tray,
        Err(error) => {
            unsafe {
                destroy_power_window(
                    power_window,
                    class_name.as_ptr(),
                    power_notification_handle,
                    power_context_ptr,
                );
            }
            return Err(error).context("Failed to create tray icon");
        }
    };

    info!("System tray icon created");

    let mut last_color = COLOR_GREY;
    let mut last_update = Instant::now();
    let mut last_menu_label = "Status: Initializing".to_string();
    let mut last_force_enable_enabled = true;
    let mut last_force_disable_enabled = true;
    let mut last_remote_model_install_label = "Enable Remote Model Installs (1 Hour)".to_string();

    // Win32 message loop
    let tray_result = unsafe {
        let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = std::mem::zeroed();

        loop {
            // Process menu events
            if let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id() == item_quit.id() {
                    info!("Exit requested via tray menu");
                    let _ = shutdown_tx.send(true);
                    break;
                } else if event.id() == item_force_enable.id() {
                    runtime.block_on(async {
                        let mut s = state.write().await;
                        s.manual_override = Some(ManualOverride::ForceEnable);
                    });
                    info!("Tray override set: force enable Ollama");
                } else if event.id() == item_force_disable.id() {
                    runtime.block_on(async {
                        let mut s = state.write().await;
                        s.manual_override = Some(ManualOverride::ForceDisable);
                    });
                    info!("Tray override set: force disable Ollama");
                } else if event.id() == item_remote_model_installs.id() {
                    runtime.block_on(async {
                        let mut s = state.write().await;
                        let now = Instant::now();
                        s.clear_expired_remote_model_install_unlock(now);
                        if s.remote_model_install_unlocked(now) {
                            s.disable_remote_model_install_unlock();
                            info!("Tray toggle disabled: remote model installs locked");
                        } else {
                            s.enable_remote_model_install_unlock(now);
                            info!(
                                "Tray toggle enabled: remote model installs unlocked for {} hour",
                                REMOTE_MODEL_INSTALL_UNLOCK_DURATION.as_secs() / 3600
                            );
                        }
                    });
                } else if event.id() == item_open_logs.id() {
                    let log_path = dirs::home_dir()
                        .unwrap_or_default()
                        .join("freecycle-verbose.log");
                    let _ = std::process::Command::new("notepad").arg(log_path).spawn();
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
                let (
                    new_color,
                    tooltip,
                    menu_label,
                    enable_force_enable,
                    enable_force_disable,
                    remote_model_install_label,
                ) = runtime.block_on(async {
                    let mut s = state.write().await;
                    let now = Instant::now();
                    if s.clear_expired_remote_model_install_unlock(now) {
                        info!("Tray toggle expired: remote model installs locked");
                    }
                    let color = status_color(&s.status, s.models_downloading);
                    let tooltip = build_tooltip(&s);
                    let menu_label = menu_status_label(&s.status, s.manual_override);
                    let enable_force_enable = force_enable_item_enabled(s.manual_override);
                    let enable_force_disable = force_disable_item_enabled(s.manual_override);
                    let remote_model_install_label = remote_model_install_menu_label(&s, now);
                    (
                        color,
                        tooltip,
                        menu_label,
                        enable_force_enable,
                        enable_force_disable,
                        remote_model_install_label,
                    )
                });

                // Only update icon if color changed
                if new_color != last_color {
                    let icon = make_icon(new_color);
                    tray.set_icon(Some(icon)).ok();
                    last_color = new_color;
                    debug!("Tray icon color updated");
                }

                tray.set_tooltip(Some(&tooltip)).ok();
                if menu_label != last_menu_label {
                    item_status.set_text(&menu_label);
                    last_menu_label = menu_label;
                }
                if enable_force_enable != last_force_enable_enabled {
                    item_force_enable.set_enabled(enable_force_enable);
                    last_force_enable_enabled = enable_force_enable;
                }
                if enable_force_disable != last_force_disable_enabled {
                    item_force_disable.set_enabled(enable_force_disable);
                    last_force_disable_enabled = enable_force_disable;
                }
                if remote_model_install_label != last_remote_model_install_label {
                    item_remote_model_installs.set_text(&remote_model_install_label);
                    last_remote_model_install_label = remote_model_install_label;
                }
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
        Ok(())
    };

    unsafe {
        destroy_power_window(
            power_window,
            class_name.as_ptr(),
            power_notification_handle,
            power_context_ptr,
        );
    }

    tray_result
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
        assert_eq!(
            status_color(&FreeCycleStatus::Available, false),
            COLOR_GREEN
        );
        assert_eq!(status_color(&FreeCycleStatus::Blocked, false), COLOR_RED);
        assert_eq!(
            status_color(
                &FreeCycleStatus::WakeDelay {
                    expires_at: Instant::now()
                },
                false
            ),
            COLOR_RED
        );
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
        assert_eq!(
            status_color(&FreeCycleStatus::Available, true),
            COLOR_YELLOW
        );
    }

    #[test]
    fn test_menu_state_helpers_respect_mutual_exclusion() {
        assert!(!force_enable_item_enabled(Some(
            ManualOverride::ForceEnable
        )));
        assert!(force_enable_item_enabled(Some(
            ManualOverride::ForceDisable
        )));
        assert!(!force_disable_item_enabled(Some(
            ManualOverride::ForceDisable
        )));
        assert!(force_disable_item_enabled(Some(
            ManualOverride::ForceEnable
        )));
    }

    #[test]
    fn test_tooltip_truncation() {
        let config = crate::config::FreeCycleConfig::default();
        let mut state = AppState::new(config);
        state.status = FreeCycleStatus::Available;
        state.ollama_running = true;
        state.vram_used_bytes = 2 * 1024 * 1024 * 1024;
        state.vram_total_bytes = 8 * 1024u64 * 1024 * 1024;
        state.model_status = vec!["Downloading llama3.1:8b-instruct-q4_K_M: 42%".to_string()];

        let tooltip = build_tooltip(&state);
        assert!(tooltip.len() <= 127);
    }

    #[test]
    fn test_tooltip_prefers_download_progress_over_agent_api_port() {
        let config = crate::config::FreeCycleConfig::default();
        let mut state = AppState::new(config);
        state.status = FreeCycleStatus::Available;
        state.ollama_running = true;
        state.vram_used_bytes = 2 * 1024 * 1024 * 1024;
        state.vram_total_bytes = 8 * 1024u64 * 1024 * 1024;
        state.model_status = vec!["Downloading llama3.1:8b-instruct-q4_K_M: 42%".to_string()];

        let tooltip = build_tooltip(&state);
        assert!(tooltip.contains("42%"));
        assert!(!tooltip.contains("Agent API:"));
    }

    #[test]
    fn test_tooltip_shows_failure_retry_text() {
        let config = crate::config::FreeCycleConfig::default();
        let mut state = AppState::new(config);
        state.status = FreeCycleStatus::Available;
        state.model_status = vec!["Failed: nomic-embed-text (retrying in 5m)".to_string()];

        let tooltip = build_tooltip(&state);
        assert!(tooltip.contains("retrying in 5m"));
    }

    #[test]
    fn test_tooltip_shows_override_context() {
        let config = crate::config::FreeCycleConfig::default();
        let mut state = AppState::new(config);
        state.status = FreeCycleStatus::Available;
        state.manual_override = Some(ManualOverride::ForceDisable);

        let tooltip = build_tooltip(&state);
        assert!(tooltip.contains("Forced Stop"));
    }

    #[test]
    fn test_tooltip_shows_wake_delay() {
        let config = crate::config::FreeCycleConfig::default();
        let mut state = AppState::new(config);
        state.status = FreeCycleStatus::WakeDelay {
            expires_at: Instant::now() + Duration::from_secs(45),
        };

        let tooltip = build_tooltip(&state);
        assert!(tooltip.contains("Wake delay"));
    }

    #[test]
    fn test_tooltip_shows_remote_model_install_unlock() {
        let config = crate::config::FreeCycleConfig::default();
        let mut state = AppState::new(config);
        state.status = FreeCycleStatus::Available;
        state.remote_model_install_unlocked_until = Some(Instant::now() + Duration::from_secs(90));

        let tooltip = build_tooltip(&state);
        assert!(tooltip.contains("Remote installs"));
    }

    #[test]
    fn test_remote_model_install_menu_label_reflects_lock_state() {
        let config = crate::config::FreeCycleConfig::default();
        let mut state = AppState::new(config);
        let now = Instant::now();

        assert_eq!(
            remote_model_install_menu_label(&state, now),
            "Enable Remote Model Installs (1 Hour)"
        );

        state.remote_model_install_unlocked_until = Some(now + Duration::from_secs(90));
        assert!(remote_model_install_menu_label(&state, now)
            .starts_with("Disable Remote Model Installs"));
    }

    #[test]
    fn test_resume_wake_delay_clears_force_enable_override() {
        let mut state = AppState::new(crate::config::FreeCycleConfig::default());
        state.status = FreeCycleStatus::Available;
        state.manual_override = Some(ManualOverride::ForceEnable);
        let now = Instant::now();

        assert!(apply_resume_wake_delay(&mut state, now, true));
        assert!(state.manual_override.is_none());
        assert!(matches!(state.status, FreeCycleStatus::WakeDelay { .. }));
        assert_eq!(
            state.wake_block_until,
            Some(now + Duration::from_secs(state.config.general.wake_delay_seconds))
        );
    }

    #[test]
    fn test_duplicate_resume_does_not_extend_wake_delay() {
        let mut state = AppState::new(crate::config::FreeCycleConfig::default());
        let first_resume_at = Instant::now();

        assert!(apply_resume_wake_delay(&mut state, first_resume_at, true));
        let first_deadline = state.wake_block_until;

        assert!(!apply_resume_wake_delay(
            &mut state,
            first_resume_at + Duration::from_secs(1),
            false,
        ));
        assert_eq!(state.wake_block_until, first_deadline);
    }

    #[test]
    fn test_resume_keeps_process_block_visible() {
        let mut state = AppState::new(crate::config::FreeCycleConfig::default());
        state.status = FreeCycleStatus::Blocked;

        assert!(apply_resume_wake_delay(&mut state, Instant::now(), true));
        assert_eq!(state.status, FreeCycleStatus::Blocked);
        assert!(state.wake_block_until.is_some());
    }

    #[test]
    fn test_resume_after_new_suspend_restarts_wake_delay() {
        let mut state = AppState::new(crate::config::FreeCycleConfig::default());
        let first_resume_at = Instant::now();
        let second_resume_at = first_resume_at + Duration::from_secs(5);

        assert!(apply_resume_wake_delay(&mut state, first_resume_at, true));
        let first_deadline = state.wake_block_until;

        assert!(apply_resume_wake_delay(&mut state, second_resume_at, true));
        assert_ne!(state.wake_block_until, first_deadline);
        assert_eq!(
            state.wake_block_until,
            Some(second_resume_at + Duration::from_secs(state.config.general.wake_delay_seconds))
        );
    }
}
