//! Start Menu shortcut (.lnk) management for FreeCycle.
//!
//! Creates and maintains a shortcut in the user's Start Menu Programs folder
//! so FreeCycle appears in the Windows Start Menu search and app list.
//!
//! Uses the `windows` crate for COM-based `IShellLinkW` / `IPersistFile`
//! operations.

use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};
use windows::core::{Interface, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, STGM,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

/// Result of checking the Start Menu shortcut state.
#[derive(Debug)]
pub enum ShortcutCheckResult {
    /// Shortcut already exists and points to the current executable.
    AlreadyCorrect,
    /// Shortcut was missing and has been created.
    Created,
    /// Shortcut exists but points to a different path (old target returned).
    Mismatched(String),
    /// Something went wrong (error message returned).
    Failed(String),
}

/// Returns the expected Start Menu shortcut path.
///
/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs\FreeCycle.lnk`
fn shortcut_path() -> Option<PathBuf> {
    dirs::config_dir().map(|roaming| {
        roaming
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("FreeCycle.lnk")
    })
}

/// Encodes a Rust string as a null-terminated UTF-16 vector.
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// RAII guard for COM initialisation on the current thread.
struct ComGuard {
    initialised: bool,
}

impl ComGuard {
    fn new() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        // S_OK or S_FALSE (already initialised) are both fine.
        let initialised = hr.is_ok();
        if !initialised {
            debug!("CoInitializeEx returned error (COM may already be initialised differently)");
        }
        Self { initialised }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialised {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

/// Reads the target path of an existing `.lnk` file, or `None` if it cannot
/// be read.
fn read_lnk_target(lnk_path: &Path) -> Option<String> {
    let _com = ComGuard::new();

    let link: IShellLinkW =
        unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.ok()?;

    let persist: windows::Win32::System::Com::IPersistFile =
        link.cast().ok()?;

    let wide_path = to_wide(&lnk_path.to_string_lossy());
    unsafe { persist.Load(PCWSTR(wide_path.as_ptr()), STGM(0)) }.ok()?;

    let mut buf = [0u16; 1024];
    unsafe {
        link.GetPath(
            &mut buf,
            std::ptr::null_mut(),
            0,
        )
    }
    .ok()?;

    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Creates (or overwrites) a `.lnk` file pointing to the given target.
fn create_lnk(lnk_path: &Path, target: &str) -> Result<(), String> {
    let _com = ComGuard::new();

    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| format!("CoCreateInstance(ShellLink): {}", e))?;

    let wide_target = to_wide(target);
    unsafe { link.SetPath(PCWSTR(wide_target.as_ptr())) }
        .map_err(|e| format!("SetPath: {}", e))?;

    let desc = to_wide("FreeCycle — GPU-aware Ollama lifecycle manager");
    unsafe { link.SetDescription(PCWSTR(desc.as_ptr())) }
        .map_err(|e| format!("SetDescription: {}", e))?;

    // Set working directory to the executable's parent folder.
    if let Some(parent) = Path::new(target).parent() {
        let wide_dir = to_wide(&parent.to_string_lossy());
        unsafe { link.SetWorkingDirectory(PCWSTR(wide_dir.as_ptr())) }
            .map_err(|e| format!("SetWorkingDirectory: {}", e))?;
    }

    let persist: windows::Win32::System::Com::IPersistFile =
        link.cast().map_err(|e| format!("QueryInterface(IPersistFile): {}", e))?;

    let wide_lnk = to_wide(&lnk_path.to_string_lossy());
    unsafe { persist.Save(PCWSTR(wide_lnk.as_ptr()), true) }
        .map_err(|e| format!("IPersistFile::Save: {}", e))?;

    Ok(())
}

/// Checks whether a Start Menu shortcut exists and is up-to-date.
///
/// - If the shortcut is missing, creates it and returns [`ShortcutCheckResult::Created`].
/// - If the shortcut exists and targets the current executable, returns
///   [`ShortcutCheckResult::AlreadyCorrect`].
/// - If the shortcut exists but targets a different path, returns
///   [`ShortcutCheckResult::Mismatched`] with the old target path.
pub fn check_and_create_shortcut() -> ShortcutCheckResult {
    let lnk = match shortcut_path() {
        Some(p) => p,
        None => return ShortcutCheckResult::Failed("Could not determine Start Menu path".into()),
    };

    let current_exe = match std::env::current_exe() {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(e) => return ShortcutCheckResult::Failed(format!("current_exe: {}", e)),
    };

    if lnk.exists() {
        match read_lnk_target(&lnk) {
            Some(existing) if paths_equal(&existing, &current_exe) => {
                debug!("Start Menu shortcut already points to current executable");
                ShortcutCheckResult::AlreadyCorrect
            }
            Some(existing) => {
                info!(
                    "Start Menu shortcut target mismatch: existing={}, current={}",
                    existing, current_exe
                );
                ShortcutCheckResult::Mismatched(existing)
            }
            None => {
                warn!("Could not read existing shortcut target; recreating");
                match create_lnk(&lnk, &current_exe) {
                    Ok(()) => {
                        info!("Recreated Start Menu shortcut: {}", lnk.display());
                        ShortcutCheckResult::Created
                    }
                    Err(e) => ShortcutCheckResult::Failed(e),
                }
            }
        }
    } else {
        match create_lnk(&lnk, &current_exe) {
            Ok(()) => {
                info!("Created Start Menu shortcut: {}", lnk.display());
                ShortcutCheckResult::Created
            }
            Err(e) => ShortcutCheckResult::Failed(e),
        }
    }
}

/// Overwrites the Start Menu shortcut to point to the current executable.
pub fn update_shortcut() -> Result<(), String> {
    let lnk = shortcut_path()
        .ok_or_else(|| "Could not determine Start Menu path".to_string())?;

    let current_exe = std::env::current_exe()
        .map_err(|e| format!("current_exe: {}", e))?
        .to_string_lossy()
        .to_string();

    create_lnk(&lnk, &current_exe)?;
    info!("Updated Start Menu shortcut: {}", lnk.display());
    Ok(())
}

/// Case-insensitive path comparison (Windows paths are case-insensitive).
fn paths_equal(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}
