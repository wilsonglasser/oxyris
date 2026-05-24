//! Taskbar unread badge.
//!
//! The frontend tracks a count of turns that completed while the window was
//! unfocused (see `badgeStore` / `taskbarBadge.ts`) and pushes it here whenever
//! it changes. Windows has no numeric taskbar badge, so — like WhatsApp — we
//! render the number into a small overlay icon on the frontend `<canvas>` and
//! set it via `set_overlay_icon`. macOS/Linux get a real numeric badge via
//! `set_badge_count`. `count == 0` clears whichever is in use.

/// Push the unread count to the taskbar.
///
/// `rgba`/`width`/`height` carry the pre-rendered overlay icon (row-major RGBA
/// straight from a canvas `getImageData`); they're only consumed on Windows.
#[tauri::command]
pub fn set_taskbar_badge(
    window: tauri::WebviewWindow,
    count: u32,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let icon = if count > 0 && !rgba.is_empty() {
            Some(tauri::image::Image::new_owned(rgba, width, height))
        } else {
            None
        };
        window.set_overlay_icon(icon).map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (rgba, width, height);
        let value = if count > 0 { Some(count as i64) } else { None };
        window.set_badge_count(value).map_err(|e| e.to_string())
    }
}
