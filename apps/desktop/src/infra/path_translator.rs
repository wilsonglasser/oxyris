//! Windows ↔ POSIX path translation inside WSL distros.
//!
//! Every WSL distro ships `wslpath` which handles the conversion properly,
//! including `C:\` ↔ `/mnt/c/`, case-fixing, and handling of `\\wsl.localhost`
//! UNC paths. We shell out to it rather than re-implementing — `wslpath`'s
//! rules drift occasionally and matching them is not worth the complexity.

use std::io::Write;
use std::process::{Command, Stdio};

use oxyris_procutil::HideConsole;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathTranslateError {
    #[error("spawn wsl.exe: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("wslpath in {distro:?} failed: {stderr}")]
    Failed { distro: String, stderr: String },
}

fn run_wslpath(distro: &str, flag: &str, path: &str) -> Result<String, PathTranslateError> {
    // wsl.exe mangles forwarded args (treats `\` as escape; some versions
    // collapse additional characters). Stream the path over stdin via a
    // quoted heredoc so no path byte ever flows through wsl.exe's arg
    // parser — bash reads the body verbatim.
    let delim = "OXYRIS_PATH_EOF_2c91";
    if path.contains(delim) || !matches!(flag, "-u" | "-w" | "-m" | "-a") {
        return Err(PathTranslateError::Failed {
            distro: distro.to_owned(),
            stderr: format!("invalid flag or path contains reserved delimiter: {flag} {path}"),
        });
    }
    let script = format!(
        "p=$(cat <<'{delim}'\n\
         {path}\n\
         {delim}\n\
         )\n\
         # Strip the leading whitespace that the line-continuation indent\n\
         # injects, plus any trailing newline that the heredoc captures.\n\
         p=${{p#\"${{p%%[![:space:]]*}}\"}}\n\
         p=${{p%\"${{p##*[![:space:]]}}\"}}\n\
         exec wslpath {flag} \"$p\"\n"
    );
    let mut child = Command::new("wsl.exe")
        .args(["-d", distro, "--", "bash", "-s"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .hide_console()
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(PathTranslateError::Failed {
            distro: distro.to_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    let translated = String::from_utf8_lossy(&out.stdout)
        .trim()
        .replace('\u{0}', "");
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
