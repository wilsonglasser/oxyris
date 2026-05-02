//! Windows ↔ POSIX path translation inside WSL distros.
//!
//! Every WSL distro ships `wslpath` which handles the conversion properly,
//! including `C:\` ↔ `/mnt/c/`, case-fixing, and handling of `\\wsl.localhost`
//! UNC paths. We shell out to it rather than re-implementing — `wslpath`'s
//! rules drift occasionally and matching them is not worth the complexity.

use std::process::Command;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathTranslateError {
    #[error("spawn wsl.exe: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("wslpath in {distro:?} failed: {stderr}")]
    Failed { distro: String, stderr: String },
}

fn run_wslpath(distro: &str, flag: &str, path: &str) -> Result<String, PathTranslateError> {
    let out = Command::new("wsl.exe")
        .args(["-d", distro, "--", "wslpath", flag, path])
        .output()?;
    if !out.status.success() {
        return Err(PathTranslateError::Failed {
            distro: distro.to_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    let translated = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Ok(translated)
}

/// Convert a Windows path (`C:\dev\proj`) into its POSIX form inside `distro`
/// (`/mnt/c/dev/proj`).
pub fn to_posix(distro: &str, windows_path: &str) -> Result<String, PathTranslateError> {
    run_wslpath(distro, "-u", windows_path)
}

/// Convert a POSIX path inside `distro` into its Windows UNC form
/// (`\\wsl.localhost\<distro>\home\user\proj`). Useful for "open in Explorer"
/// and absolutely nothing else — hot-path ops must stay inside the distro via
/// the agent (see `PLAN.md` §13).
pub fn to_windows(distro: &str, posix_path: &str) -> Result<String, PathTranslateError> {
    run_wslpath(distro, "-w", posix_path)
}
