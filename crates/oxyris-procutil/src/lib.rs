//! Tiny cross-platform helper for hiding the console window that Windows
//! attaches to console-subsystem children spawned by a GUI app.
//!
//! On Windows, `std::process::Command` and `tokio::process::Command` both
//! inherit the parent's console handle by default. When the parent is a
//! GUI app (Tauri release) it has none, so Windows allocates a fresh
//! console for every child — that's the visible flashing terminal. Setting
//! `CREATE_NO_WINDOW` (0x0800_0000) on the child's creation flags suppresses
//! that allocation.
//!
//! On non-Windows targets the helpers are no-ops so callers don't need
//! `cfg(windows)` at every spawn site.

#![forbid(unsafe_code)]

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Extension trait that hides the console for a child process on Windows
/// and is a no-op everywhere else. Implemented for both `std` and `tokio`
/// `Command` types so the same `.hide_console()` chain works for either.
pub trait HideConsole {
    fn hide_console(&mut self) -> &mut Self;
}

impl HideConsole for std::process::Command {
    fn hide_console(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

impl HideConsole for tokio::process::Command {
    fn hide_console(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}
