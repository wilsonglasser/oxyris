//! Provider installation discovery — given an environment, figure out
//! whether Claude (or any other registered provider) is installed there,
//! its path, and its version. Used by Settings to surface "Claude
//! (Windows): not authenticated" without spawning a real session.
//!
//! For WSL, we filter out `/mnt/*` paths — those are interop shims of the
//! Windows install seen from inside the distro and using them defeats the
//! whole environment-routing model.

use std::process::Command as StdCommand;
use std::time::Duration;

use oxyris_core::Environment;
use oxyris_procutil::HideConsole;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredInstall {
    pub provider_id: String,
    pub environment: Environment,
    pub path: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
    /// True when the install we found is the WSL-side shim of the Windows
    /// binary (path under `/mnt/c/...`). Surfaced separately so Settings
    /// can suggest installing claude natively in the distro.
    pub is_interop_shim: bool,
}

const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn discover_claude(env: Environment) -> DiscoveredInstall {
    match env {
        Environment::Local => discover_windows().await,
        Environment::Wsl { ref distro } => discover_wsl(distro.clone(), env.clone()).await,
    }
}

async fn discover_windows() -> DiscoveredInstall {
    let resolved = which::which("claude")
        .or_else(|_| which::which("claude.cmd"))
        .or_else(|_| which::which("claude.exe"));

    let path = match resolved {
        Ok(p) => p,
        Err(e) => {
            return DiscoveredInstall {
                provider_id: "claude".into(),
                environment: Environment::Local,
                path: None,
                version: None,
                error: Some(format!("not on PATH: {e}")),
                is_interop_shim: false,
            };
        }
    };

    let version = run_version_windows(&path).await;
    DiscoveredInstall {
        provider_id: "claude".into(),
        environment: Environment::Local,
        path: Some(path.to_string_lossy().into_owned()),
        version: version.clone().ok(),
        error: version.err(),
        is_interop_shim: false,
    }
}

async fn run_version_windows(path: &std::path::Path) -> Result<String, String> {
    let is_batch = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e.to_ascii_lowercase().as_str(), "cmd" | "bat"))
        .unwrap_or(false);

    let mut cmd = if is_batch {
        let mut c = Command::new("cmd.exe");
        c.arg("/C");
        c.arg(path.as_os_str());
        c
    } else {
        Command::new(path.as_os_str())
    };
    cmd.arg("--version");
    cmd.kill_on_drop(true);
    cmd.hide_console();

    let result = tokio::time::timeout(VERSION_TIMEOUT, cmd.output()).await;
    match result {
        Ok(Ok(out)) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_owned()),
        Ok(Ok(out)) => Err(format!(
            "claude --version exit {}: {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Ok(Err(e)) => Err(format!("spawn: {e}")),
        Err(_) => Err("--version timed out".into()),
    }
}

async fn discover_wsl(distro: String, env: Environment) -> DiscoveredInstall {
    // `bash -ilc` runs an interactive login shell so `~/.bashrc`, nvm init,
    // pyenv, etc. populate PATH just as they do in a real terminal. `sh -lc`
    // on Debian/Ubuntu is `dash` and misses most user PATH setups.
    //
    // `which` is what the user explicitly asks for — we run it first, print a
    // separator, then run `--version` so both pieces land in one roundtrip.
    let script = "which claude || true; printf '\\n---\\n'; \
                  claude --version 2>/dev/null || true";
    let result = tokio::task::spawn_blocking({
        let distro = distro.clone();
        move || {
            StdCommand::new("wsl.exe")
                .args(["-d", &distro, "--", "bash", "-ilc", script])
                .hide_console()
                .output()
        }
    })
    .await
    .map_err(|e| format!("join: {e}"))
    .and_then(|r| r.map_err(|e| format!("spawn: {e}")));

    match result {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut parts = text.split("\n---\n");
            let path = parts.next().unwrap_or("").trim().to_owned();
            let version_block = parts.next().unwrap_or("").trim().to_owned();

            if path.is_empty() {
                return DiscoveredInstall {
                    provider_id: "claude".into(),
                    environment: env,
                    path: None,
                    version: None,
                    error: Some(format!(
                        "claude not on PATH inside {distro}: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )),
                    is_interop_shim: false,
                };
            }
            let is_interop_shim = path.starts_with("/mnt/");
            let version = if version_block.is_empty() {
                None
            } else {
                Some(version_block.lines().next().unwrap_or("").trim().to_owned())
            };
            DiscoveredInstall {
                provider_id: "claude".into(),
                environment: env,
                path: Some(path),
                version,
                error: None,
                is_interop_shim,
            }
        }
        Err(e) => DiscoveredInstall {
            provider_id: "claude".into(),
            environment: env,
            path: None,
            version: None,
            error: Some(e),
            is_interop_shim: false,
        },
    }
}
