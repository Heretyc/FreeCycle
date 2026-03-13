//! Windows balloon-tip notifications for state change events.
//!
//! Uses `Shell_NotifyIconW` with a dedicated short-lived tray icon entry
//! (icon ID 100) to show balloon-style notifications on significant
//! FreeCycle state transitions. All errors are silently ignored; this is
//! a best-effort, nice-to-have feature.

use std::time::Duration;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::{
    NIF_INFO, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIIF_NOSOUND, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};

/// Tray icon entry ID used exclusively for balloon notifications.
///
/// Kept distinct from whatever ID the `tray-icon` crate uses internally (typically 1).
const NOTIFICATION_ICON_ID: u32 = 100;

/// Category of balloon notification, controlling the icon shown inside the balloon.
pub enum BalloonKind {
    Info,
    Warning,
    Error,
}

/// Copies `s` as UTF-16 into a fixed-size array, null-terminating and truncating as needed.
fn fill_wide<const N: usize>(s: &str) -> [u16; N] {
    let mut buf = [0u16; N];
    let chars: Vec<u16> = s.encode_utf16().collect();
    let len = chars.len().min(N.saturating_sub(1));
    buf[..len].copy_from_slice(&chars[..len]);
    buf
}

/// Shows a Windows balloon-tip notification associated with the given HWND.
///
/// Creates a short-lived tray icon entry (ID 100) on `hwnd`, fires the balloon,
/// then removes the entry after 8 seconds on a background thread. All Win32
/// failures are silently ignored.
///
/// # Arguments
///
/// * `hwnd` - A valid Win32 window handle (e.g., the FreeCycle power-event window).
/// * `title` - Balloon title (max 63 UTF-16 code units, truncated if longer).
/// * `body` - Balloon body text (max 255 UTF-16 code units, truncated if longer).
/// * `kind` - Icon shown inside the balloon: info, warning, or error.
pub fn show_balloon(hwnd: HWND, title: &str, body: &str, kind: BalloonKind) {
    let tip_w: [u16; 128] = fill_wide("FreeCycle");
    let title_w: [u16; 64] = fill_wide(title);
    let body_w: [u16; 256] = fill_wide(body);

    let info_flags = match kind {
        BalloonKind::Info => NIIF_INFO | NIIF_NOSOUND,
        BalloonKind::Warning => NIIF_WARNING | NIIF_NOSOUND,
        BalloonKind::Error => NIIF_ERROR,
    };

    // SAFETY: All fields are set explicitly; zeroed() satisfies the Anonymous union and GUID.
    unsafe {
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = NOTIFICATION_ICON_ID;

        // Register a minimal tray entry (no icon image; tooltip only).
        data.uFlags = NIF_TIP;
        data.szTip = tip_w;
        Shell_NotifyIconW(NIM_ADD, &data);

        // Set the balloon content on the same entry.
        data.uFlags = NIF_INFO;
        data.szInfoTitle = title_w;
        data.szInfo = body_w;
        data.dwInfoFlags = info_flags;
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }

    // Remove the temporary icon entry after the balloon has had time to display.
    // Cast HWND to usize to cross the thread boundary (HWND is a raw pointer and not Send).
    // The value is only used to call Shell_NotifyIconW; it is not dereferenced.
    let hwnd_usize = hwnd as usize;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(8));
        // SAFETY: Shell_NotifyIconW validates the HWND and returns FALSE if it is
        // no longer valid (e.g., destroyed before the thread wakes). We ignore the result.
        unsafe {
            let mut del: NOTIFYICONDATAW = std::mem::zeroed();
            del.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            del.hWnd = hwnd_usize as HWND;
            del.uID = NOTIFICATION_ICON_ID;
            Shell_NotifyIconW(NIM_DELETE, &del);
        }
    });
}
