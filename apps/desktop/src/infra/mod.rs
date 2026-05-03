//! Infrastructure — the non-pure edge of the app: storage, process spawns,
//! path translation, filesystem walks, git. Anything that touches the outside
//! world lives here.

pub mod agent_pool;
pub mod checkpoint;
pub mod docker_cleanup;
pub mod dotenv_render;
pub mod env_template;
pub mod environments;
pub mod event_store;
pub mod fs;
pub mod fs_watcher;
pub mod git;
pub mod indexing;
pub mod language_packs;
pub mod lsp;
pub mod lsp_bridge;
pub mod mcp;
pub mod observability;

/// Pick which distro a workspace path belongs to.
///
/// UNC paths (`\\wsl.localhost\<distro>\...` or the legacy `\\wsl$\...`)
/// carry the distro name in segment two — extract directly. Bare POSIX
/// paths (`/home/...`) lose that info on the wire, so we fall back to
/// the system default distro from `wsl.exe --list --quiet`. Multi-distro
/// users with bare-POSIX projects need `wsl --set-default <distro>`.
pub fn wsl_distro_for_path(path: &std::path::Path) -> Option<String> {
    let s = path.to_string_lossy();
    for prefix in [
        r"\\wsl.localhost\",
        r"\\wsl$\",
        "//wsl.localhost/",
        "//wsl$/",
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            let distro = rest.split(['\\', '/']).next()?;
            if !distro.is_empty() {
                return Some(distro.to_owned());
            }
        }
    }
    if s.starts_with('/') {
        return default_wsl_distro();
    }
    None
}

fn default_wsl_distro() -> Option<String> {
    use std::process::Command;
    let out = Command::new("wsl.exe")
        .arg("--list")
        .arg("--quiet")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = decode_wsl_output(&out.stdout);
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|s| s.to_owned())
}

/// `wsl.exe` emits UTF-16LE on stdout. Decode it; if BOM-less, fall back
/// to UTF-8 lossy.
pub fn decode_wsl_output_for_command(bytes: &[u8]) -> String {
    decode_wsl_output(bytes)
}

fn decode_wsl_output(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        // UTF-16LE with BOM
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.len().is_multiple_of(2)
        && bytes.iter().step_by(2).filter(|b| **b == 0).count() > bytes.len() / 4
    {
        // Looks like UTF-16LE without BOM
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod wsl_distro_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn unc_wsl_localhost_extracts_distro() {
        assert_eq!(
            wsl_distro_for_path(Path::new(r"\\wsl.localhost\Ubuntu-22.04\home\me\proj")),
            Some("Ubuntu-22.04".to_owned())
        );
    }

    #[test]
    fn legacy_wsl_dollar_extracts_distro() {
        assert_eq!(
            wsl_distro_for_path(Path::new(r"\\wsl$\Debian\srv\app")),
            Some("Debian".to_owned())
        );
    }

    #[test]
    fn forward_slash_unc_works() {
        assert_eq!(
            wsl_distro_for_path(Path::new("//wsl.localhost/Alpine/etc")),
            Some("Alpine".to_owned())
        );
    }

    #[test]
    fn windows_drive_returns_none() {
        assert_eq!(wsl_distro_for_path(Path::new(r"C:\Users\me\proj")), None);
    }
}

pub mod path_translator;
pub mod projections;
pub mod provider_discovery;
pub mod pty;
pub mod session_supervisor;
