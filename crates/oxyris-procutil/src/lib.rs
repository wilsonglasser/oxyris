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

/// Program + fixed argument prefix for running a shell-script *string* on the
/// host. Append the script as the final argument.
///
/// - Windows host → `("cmd.exe", ["/C"])`
/// - Unix host    → `("sh", ["-c"])`
///
/// Works for both `std::process::Command` and `tokio::process::Command`:
/// `let (sh, pre) = host_shell(); Command::new(sh).args(pre).arg(script)`.
/// Used only for `Environment::Local` spawns — `Wsl` routes through `wsl.exe`.
pub fn host_shell() -> (&'static str, &'static [&'static str]) {
    #[cfg(windows)]
    {
        ("cmd.exe", &["/C"])
    }
    #[cfg(not(windows))]
    {
        ("sh", &["-c"])
    }
}

/// Kill a process **and its entire descendant tree**.
///
/// `child.kill()` (portable-pty, `std::process`) terminates only the direct
/// child. When that child is a shell running `cargo run`, the actual app is a
/// *grandchild*: killing the shell leaves the app orphaned and still running.
/// A real terminal kills the whole tree on close; this mirrors that.
///
/// - Windows → `taskkill /PID <pid> /T /F` (`/T` walks the tree, `/F` forces).
///   The console window is hidden so no terminal flashes during shutdown.
/// - Unix    → `kill -KILL -<pid>`: the negative pid targets the *process
///   group*. A PTY child is its own session/group leader (the slave becomes
///   its controlling terminal), so `pgid == pid` and the whole foreground
///   tree under the shell goes down with it.
///
/// Best-effort: a process that already exited yields a non-zero status which
/// we ignore. Returns once the kill command has been spawned and waited on.
pub fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .hide_console()
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .output();
    }
}

/// The current user's home directory, read from the host's native env var.
///
/// - Windows host → `%USERPROFILE%`
/// - Unix host    → `$HOME`
pub fn home_dir() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(Into::into)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(Into::into)
    }
}
