//! Environment discovery — figure out which target namespaces exist on this
//! machine (Windows itself is always present; WSL distros come from
//! `wsl.exe --list --verbose`).
//!
//! `wsl.exe` output is UTF-16 LE with a BOM and contains internal distros
//! (`docker-desktop`, `docker-desktop-data`) that are not real dev targets,
//! so we strip those out.

use std::process::{Command, Output};

use oxyris_core::Environment;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EnvironmentsError {
    #[error("failed to run wsl.exe: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("wsl.exe returned non-zero status")]
    NonZero { status: Option<i32>, stderr: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvironmentEntry {
    #[serde(flatten)]
    pub environment: Environment,
    pub state: Option<String>,
    pub version: Option<String>,
    pub is_default: bool,
}

/// Internal WSL distros that ship with Docker Desktop — not project targets.
/// Ordered check is against exact names (case-insensitive).
const HIDDEN_DISTROS: &[&str] = &["docker-desktop", "docker-desktop-data"];

fn is_hidden(name: &str) -> bool {
    HIDDEN_DISTROS
        .iter()
        .any(|hidden| name.eq_ignore_ascii_case(hidden))
}

/// Discover all environments the app can host projects in.
///
/// Always includes [`Environment::Windows`] first. Failure to enumerate WSL
/// is **not** fatal — it just means "no WSL detected on this box" and we
/// return only Windows.
pub fn environments_list() -> Vec<EnvironmentEntry> {
    let mut out = vec![EnvironmentEntry {
        environment: Environment::Windows,
        state: None,
        version: None,
        is_default: true,
    }];

    match run_wsl_list() {
        Ok(distros) => {
            for d in distros {
                if !is_hidden(&d.name) {
                    out.push(EnvironmentEntry {
                        environment: Environment::Wsl {
                            distro: d.name.clone(),
                        },
                        state: Some(d.state),
                        version: Some(d.version),
                        is_default: d.is_default,
                    });
                }
            }
        }
        Err(e) => {
            tracing::debug!(error = %e, "wsl.exe --list --verbose failed; skipping WSL discovery");
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WslDistro {
    pub name: String,
    pub state: String,
    pub version: String,
    pub is_default: bool,
}

fn run_wsl_list() -> Result<Vec<WslDistro>, EnvironmentsError> {
    let out = Command::new("wsl.exe")
        .args(["--list", "--verbose"])
        .output()?;
    check_status(&out)?;
    Ok(parse_wsl_list(&out.stdout))
}

fn check_status(out: &Output) -> Result<(), EnvironmentsError> {
    if !out.status.success() {
        return Err(EnvironmentsError::NonZero {
            status: out.status.code(),
            stderr: decode_output(&out.stderr),
        });
    }
    Ok(())
}

/// Decode `wsl.exe` stdout/stderr. It usually emits UTF-16 LE — sometimes
/// with a BOM, sometimes not (depends on Windows build / locale). When there
/// is no BOM we sniff for UTF-16 by looking at the null-byte cadence. Tests
/// pass plain ASCII, which we treat as UTF-8.
fn decode_output(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let (cow, _, _) = encoding_rs::UTF_16LE.decode(rest);
        return cow.into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let (cow, _, _) = encoding_rs::UTF_16BE.decode(rest);
        return cow.into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if looks_like_utf16le(bytes) {
        let (cow, _, _) = encoding_rs::UTF_16LE.decode(bytes);
        return cow.into_owned();
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Heuristic: is this byte stream likely UTF-16 LE encoded ASCII text without
/// a BOM? Looks at the first chunk and checks that the high byte of every
/// 16-bit unit is zero — that's true for any ASCII content under UTF-16 LE
/// and almost never true for genuine UTF-8.
fn looks_like_utf16le(bytes: &[u8]) -> bool {
    if bytes.len() < 4 || !bytes.len().is_multiple_of(2) {
        return false;
    }
    let sample = &bytes[..bytes.len().min(64)];
    let pairs = sample.chunks_exact(2);
    let total = pairs.len();
    if total == 0 {
        return false;
    }
    let zeros = sample.chunks_exact(2).filter(|p| p[1] == 0).count();
    // Demand ≥80% high-byte zeros — comfortable margin against random binary.
    zeros * 5 >= total * 4
}

fn parse_wsl_list(bytes: &[u8]) -> Vec<WslDistro> {
    let text = decode_output(bytes);
    let mut out = Vec::new();

    for (idx, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim_end();
        if line.is_empty() {
            continue;
        }

        // Header looks like "  NAME  STATE  VERSION" — always first non-empty line.
        if idx == 0 && line.trim_start().starts_with("NAME") {
            continue;
        }

        let trimmed_start = line.trim_start_matches(' ');
        let (is_default, rest) = if let Some(rest) = trimmed_start.strip_prefix("* ") {
            (true, rest)
        } else {
            (false, trimmed_start)
        };

        let mut iter = rest.split_whitespace();
        let Some(name) = iter.next() else {
            continue;
        };
        let state = iter.next().unwrap_or("").to_owned();
        let version = iter.next().unwrap_or("").to_owned();

        // Skip duplicate headers if any tool ever repeats them.
        if name.eq_ignore_ascii_case("NAME") {
            continue;
        }

        out.push(WslDistro {
            name: name.to_owned(),
            state,
            version,
            is_default,
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16_bytes(s: &str) -> Vec<u8> {
        let mut v = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }

    #[test]
    fn parses_typical_output_with_default_marker() {
        let raw = "  NAME                      STATE           VERSION\n\
                   * Ubuntu                    Running         2\n  \
                     Debian                    Stopped         2\n  \
                     docker-desktop            Stopped         2\n";
        let distros = parse_wsl_list(&utf16_bytes(raw));
        // The parser returns every distro; hiding is done in the caller.
        assert!(distros.iter().any(|d| d.name == "Ubuntu" && d.is_default));
        assert!(distros.iter().any(|d| d.name == "Debian" && !d.is_default));
        assert!(distros.iter().any(|d| d.name == "docker-desktop"));
    }

    #[test]
    fn handles_output_without_bom() {
        let raw = "  NAME    STATE    VERSION\n  Ubuntu  Running  2\n";
        let distros = parse_wsl_list(raw.as_bytes());
        assert_eq!(distros.len(), 1);
        assert_eq!(distros[0].name, "Ubuntu");
    }

    #[test]
    fn handles_utf16le_without_bom() {
        // Newer wsl.exe builds drop the BOM but still emit UTF-16 LE.
        let raw = "  NAME    STATE    VERSION\n* Ubuntu  Running  2\n";
        let mut bytes: Vec<u8> = Vec::new();
        for u in raw.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let distros = parse_wsl_list(&bytes);
        assert_eq!(distros.len(), 1);
        assert_eq!(distros[0].name, "Ubuntu");
        assert!(distros[0].is_default);
    }

    #[test]
    fn is_hidden_matches_case_insensitively() {
        assert!(is_hidden("docker-desktop"));
        assert!(is_hidden("Docker-Desktop"));
        assert!(is_hidden("docker-desktop-data"));
        assert!(!is_hidden("Ubuntu"));
    }

    #[test]
    fn environments_list_always_includes_windows_first() {
        let envs = environments_list();
        assert!(matches!(envs[0].environment, Environment::Windows));
        assert!(envs[0].is_default);
    }

    #[test]
    fn environments_list_filters_docker_desktop() {
        // We can't mock wsl.exe here, but we can test the post-filter
        // indirectly: the function must never surface a hidden distro even if
        // present.
        for e in environments_list() {
            if let Environment::Wsl { distro } = &e.environment {
                assert!(!is_hidden(distro), "leaked hidden distro: {distro}");
            }
        }
    }
}
