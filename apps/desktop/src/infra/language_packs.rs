//! "Language packs" — presented to the user as plug-in style toggles, but
//! implemented as a static registry of LSP servers we know how to install.
//! Each entry knows two things: how to detect whether it's installed, and
//! how to install it.
//!
//! The registry lives in code (see [`registry`]). Adding a new language is
//! a PR appending an entry. No marketplace, no extension API, no sandbox —
//! deliberately, so we ship value early without committing to a plugin
//! contract we'd be stuck supporting forever.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use oxyris_procutil::HideConsole;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

/// Cap how long an `npm i -g` / `bun i -g` is allowed to take. intelephense's
/// post-install once hung indefinitely behind a PowerShell prompt — better
/// to surface that than spin a chip forever.
const NPM_INSTALL_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Error)]
pub enum PackError {
    #[error("unknown language pack: {0}")]
    Unknown(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("download: {0}")]
    Download(String),
    #[error("install command failed: {0}")]
    Command(String),
    #[error("dependency missing: {0}")]
    DependencyMissing(String),
}

/// Where the install lives on disk for a managed binary. Resolved against
/// `<data_dir>/lsp/`.
pub fn managed_lsp_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("lsp")
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // Manual is reserved for future packs without a programmatic install path.
pub enum InstallMethod {
    /// Pull a release asset from GitHub and (optionally) decompress it.
    GithubRelease {
        repo: &'static str,
        asset_name: &'static str,
        /// `Gzip` decompresses the asset; `Raw` writes it verbatim.
        encoding: AssetEncoding,
        /// Where to put the resulting file under `<data_dir>/lsp/`.
        binary_filename: &'static str,
    },
    /// `npm i -g <package>` — falls back to `bun i -g` when bun is on PATH
    /// (faster + doesn't need npm). Requires Node-or-bun installed.
    NpmGlobal { package: &'static str },
    /// We don't try to install — just describe the manual command.
    Manual {
        instruction: &'static str,
        url: &'static str,
    },
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // Raw is reserved for assets that ship as a bare binary (no compression).
pub enum AssetEncoding {
    /// Asset is a bare binary; write verbatim.
    Raw,
    /// Asset is gzipped; decompress before writing.
    Gzip,
    /// Asset is a zip archive; extract one entry by name.
    ZipEntry { name: &'static str },
}

/// A "language pack" entry. Static metadata; see [`registry`] for the list.
#[derive(Debug, Clone)]
pub struct Pack {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    /// Map back to the `oxyris-lsp` language enum so the LSP manager can
    /// route file extensions to the right server.
    pub lsp_language: oxyris_lsp::LspLanguage,
    /// Binary name in the managed dir (or on PATH for non-managed installs).
    pub binary_name: &'static str,
    pub install: InstallMethod,
}

/// Hand-curated registry. To add a language: append an entry here.
pub fn registry() -> &'static [Pack] {
    &[
        Pack {
            id: "rust",
            display_name: "Rust",
            description: "rust-analyzer — official Rust LSP, downloaded from GitHub releases.",
            lsp_language: oxyris_lsp::LspLanguage::Rust,
            binary_name: "rust-analyzer.exe",
            install: InstallMethod::GithubRelease {
                repo: "rust-lang/rust-analyzer",
                // rust-analyzer ships Windows builds as `.zip` (the older
                // `.gz` URL pattern returned 404 from late-2025 onward).
                asset_name: "rust-analyzer-x86_64-pc-windows-msvc.zip",
                encoding: AssetEncoding::ZipEntry {
                    name: "rust-analyzer.exe",
                },
                binary_filename: "rust-analyzer.exe",
            },
        },
        Pack {
            id: "typescript",
            display_name: "TypeScript / JavaScript",
            description: "typescript-language-server — handles both TS and JS, installed globally via npm/bun.",
            lsp_language: oxyris_lsp::LspLanguage::TypeScriptJavaScript,
            binary_name: "typescript-language-server",
            install: InstallMethod::NpmGlobal {
                package: "typescript-language-server typescript",
            },
        },
        Pack {
            id: "php",
            display_name: "PHP",
            description: "intelephense — most polished PHP language server, installed via npm/bun.",
            lsp_language: oxyris_lsp::LspLanguage::Php,
            binary_name: "intelephense",
            install: InstallMethod::NpmGlobal {
                package: "intelephense",
            },
        },
    ]
}

pub fn find(id: &str) -> Option<&'static Pack> {
    registry().iter().find(|p| p.id == id)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStatus {
    /// We can find the binary either in `<data_dir>/lsp/` or on PATH.
    Installed { source: InstallSource, path: String },
    /// Nothing on disk, nothing on PATH.
    NotInstalled,
    /// Currently downloading / running install command.
    Installing { progress: u32 },
    /// Last attempt failed. UI shows the message + "retry".
    Failed { message: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallSource {
    Managed,
    Path,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WslInstallInfo {
    pub distro: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackRow {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub lsp_language: &'static str,
    pub install_method: &'static str,
    pub status: InstallStatus,
    /// Per-distro WSL installs the user has performed via Oxyris.
    /// Persisted across restarts in `<data_dir>/language_packs_wsl.json`.
    pub wsl_installs: Vec<WslInstallInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum PackProgressEvent {
    Started {
        id: &'static str,
    },
    Progress {
        id: &'static str,
        bytes: u64,
        total: Option<u64>,
    },
    Done {
        id: &'static str,
        path: String,
    },
    Failed {
        id: &'static str,
        message: String,
    },
}

/// Single coordinator for installs. Holds an in-flight set so concurrent
/// `install_pack` calls for the same pack collapse to one job.
pub struct LanguagePacksService {
    app: AppHandle,
    data_dir: PathBuf,
    in_flight: Mutex<std::collections::HashSet<String>>,
    last_failure: Mutex<std::collections::HashMap<String, String>>,
    /// `pack_id -> [{distro, path}]`. Records every successful WSL install
    /// so the UI can show "[Ubuntu] /usr/bin/intelephense" alongside the
    /// Windows-side path. Persisted to `<data_dir>/language_packs_wsl.json`.
    wsl_installs: Mutex<std::collections::HashMap<String, Vec<WslInstallInfo>>>,
}

impl LanguagePacksService {
    pub fn new(app: AppHandle, data_dir: PathBuf) -> Self {
        let wsl_installs = load_wsl_installs(&data_dir);
        Self {
            app,
            data_dir,
            in_flight: Mutex::new(std::collections::HashSet::new()),
            last_failure: Mutex::new(std::collections::HashMap::new()),
            wsl_installs: Mutex::new(wsl_installs),
        }
    }

    /// Snapshot of every pack with its current status. Cheap to call —
    /// just FS exists checks and `which::which` lookups.
    pub async fn list(&self) -> Vec<PackRow> {
        let in_flight = self.in_flight.lock().await;
        let last_failure = self.last_failure.lock().await;
        let wsl_installs = self.wsl_installs.lock().await;
        registry()
            .iter()
            .map(|p| {
                let status = if in_flight.contains(p.id) {
                    InstallStatus::Installing { progress: 0 }
                } else if let Some(msg) = last_failure.get(p.id) {
                    InstallStatus::Failed {
                        message: msg.clone(),
                    }
                } else {
                    self.detect_status(p)
                };
                PackRow {
                    id: p.id,
                    display_name: p.display_name,
                    description: p.description,
                    lsp_language: p.lsp_language.id(),
                    install_method: install_method_label(&p.install),
                    status,
                    wsl_installs: wsl_installs.get(p.id).cloned().unwrap_or_default(),
                }
            })
            .collect()
    }

    async fn record_wsl_install(&self, pack_id: &str, distro: &str, path: &str) {
        let mut map = self.wsl_installs.lock().await;
        let entries = map.entry(pack_id.to_owned()).or_default();
        if let Some(existing) = entries.iter_mut().find(|e| e.distro == distro) {
            existing.path = path.to_owned();
        } else {
            entries.push(WslInstallInfo {
                distro: distro.to_owned(),
                path: path.to_owned(),
            });
        }
        let snapshot = map.clone();
        drop(map);
        if let Err(e) = persist_wsl_installs(&self.data_dir, &snapshot) {
            tracing::warn!(error = %e, "language_pack: failed to persist wsl_installs map");
        }
    }

    /// Resolve a pack's binary path. Prefers the managed copy in
    /// `<data_dir>/lsp/<binary>` over PATH so a freshly installed pack
    /// wins even when the user has an older one on PATH.
    pub fn resolved_binary(&self, pack: &Pack) -> Option<PathBuf> {
        let managed = managed_lsp_dir(&self.data_dir).join(pack.binary_name);
        if managed.exists() {
            return Some(managed);
        }
        find_external_binary(pack.binary_name)
    }

    fn detect_status(&self, pack: &Pack) -> InstallStatus {
        let managed = managed_lsp_dir(&self.data_dir).join(pack.binary_name);
        if managed.exists() {
            return InstallStatus::Installed {
                source: InstallSource::Managed,
                path: managed.to_string_lossy().into_owned(),
            };
        }
        if let Some(p) = find_external_binary(pack.binary_name) {
            return InstallStatus::Installed {
                source: InstallSource::Path,
                path: p.to_string_lossy().into_owned(),
            };
        }
        InstallStatus::NotInstalled
    }

    /// Drive the install end-to-end. Idempotent: if already installing,
    /// the duplicate call returns immediately. Emits `language_pack:status`
    /// events the UI watches for live updates.
    pub async fn install(self: &Arc<Self>, id: &str) -> Result<(), PackError> {
        let pack = find(id).ok_or_else(|| PackError::Unknown(id.to_owned()))?;

        // Dedupe in-flight.
        {
            let mut in_flight = self.in_flight.lock().await;
            if in_flight.contains(id) {
                return Ok(());
            }
            in_flight.insert(id.to_owned());
        }
        // Clear any previous failure so we don't show stale red.
        self.last_failure.lock().await.remove(id);

        self.emit(PackProgressEvent::Started { id: pack.id });

        let me = self.clone();
        let pack_owned = pack.clone();
        tauri::async_runtime::spawn(async move {
            let result = match &pack_owned.install {
                InstallMethod::GithubRelease {
                    repo,
                    asset_name,
                    encoding,
                    binary_filename,
                } => {
                    me.install_github(pack_owned.id, repo, asset_name, encoding, binary_filename)
                        .await
                }
                InstallMethod::NpmGlobal { package } => {
                    me.install_npm(pack_owned.id, package).await
                }
                InstallMethod::Manual { instruction, url } => Err(PackError::DependencyMissing(
                    format!("{instruction} ({url})"),
                )),
            };

            me.in_flight.lock().await.remove(pack_owned.id);

            match result {
                Ok(path) => {
                    me.emit(PackProgressEvent::Done {
                        id: pack_owned.id,
                        path,
                    });
                }
                Err(e) => {
                    let msg = e.to_string();
                    me.last_failure
                        .lock()
                        .await
                        .insert(pack_owned.id.to_owned(), msg.clone());
                    me.emit(PackProgressEvent::Failed {
                        id: pack_owned.id,
                        message: msg,
                    });
                }
            }
        });

        Ok(())
    }

    /// Remove the managed copy. PATH-resolved installs are not touched —
    /// we never installed them, we don't uninstall them.
    pub async fn uninstall(&self, id: &str) -> Result<(), PackError> {
        let pack = find(id).ok_or_else(|| PackError::Unknown(id.to_owned()))?;
        let path = managed_lsp_dir(&self.data_dir).join(pack.binary_name);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        self.last_failure.lock().await.remove(id);
        Ok(())
    }

    async fn install_github(
        &self,
        pack_id: &'static str,
        repo: &str,
        asset_name: &str,
        encoding: &AssetEncoding,
        binary_filename: &str,
    ) -> Result<String, PackError> {
        // Latest release URL — let GitHub redirect to the actual asset.
        let url = format!("https://github.com/{repo}/releases/latest/download/{asset_name}",);
        tracing::info!(pack = pack_id, %url, "language_pack: starting github download");
        let client = reqwest::Client::builder()
            .user_agent(concat!("oxyris/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| PackError::Download(e.to_string()))?;

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| PackError::Download(e.to_string()))?
            .error_for_status()
            .map_err(|e| PackError::Download(e.to_string()))?;
        let total = resp.content_length();
        let mut stream = resp.bytes_stream();

        let dir = managed_lsp_dir(&self.data_dir);
        tokio::fs::create_dir_all(&dir).await?;
        let target = dir.join(binary_filename);
        let staging = dir.join(format!("{binary_filename}.partial"));

        // Strategy depends on encoding. `Raw` writes directly to the
        // staging file as bytes arrive. `Gzip` and `ZipEntry` need the
        // whole stream before decoding, so we buffer.
        let needs_buffer = !matches!(encoding, AssetEncoding::Raw);
        let mut bytes_in: u64 = 0;
        let mut staging_file = if needs_buffer {
            None
        } else {
            Some(tokio::fs::File::create(&staging).await?)
        };
        let mut buffered: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| PackError::Download(e.to_string()))?;
            bytes_in += chunk.len() as u64;
            if let Some(file) = staging_file.as_mut() {
                file.write_all(&chunk).await?;
            } else {
                buffered.extend_from_slice(&chunk);
            }
            self.emit(PackProgressEvent::Progress {
                id: pack_id,
                bytes: bytes_in,
                total,
            });
        }

        // Finalize per-encoding: decode/extract into the staging file.
        match encoding {
            AssetEncoding::Raw => {
                if let Some(mut f) = staging_file.take() {
                    f.flush().await?;
                }
            }
            AssetEncoding::Gzip => {
                let decoded = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
                    use flate2::read::GzDecoder;
                    use std::io::Read;
                    let mut d = GzDecoder::new(&buffered[..]);
                    let mut out = Vec::new();
                    d.read_to_end(&mut out)?;
                    Ok(out)
                })
                .await
                .map_err(|e| PackError::Download(e.to_string()))??;
                let mut f = tokio::fs::File::create(&staging).await?;
                f.write_all(&decoded).await?;
                f.flush().await?;
            }
            AssetEncoding::ZipEntry { name } => {
                // Extract just the named entry from the zip into a Vec, then
                // write it to staging. zip crate is sync — bounce off the
                // runtime so the chip update doesn't stutter.
                let entry_name = (*name).to_owned();
                let extracted =
                    tokio::task::spawn_blocking(move || -> Result<Vec<u8>, PackError> {
                        use std::io::Cursor;
                        use std::io::Read;
                        let cursor = Cursor::new(buffered);
                        let mut archive = zip::ZipArchive::new(cursor)
                            .map_err(|e| PackError::Download(format!("zip archive: {e}")))?;
                        let mut entry = archive.by_name(&entry_name).map_err(|e| {
                            PackError::Download(format!(
                                "entry `{entry_name}` not found in zip: {e}"
                            ))
                        })?;
                        let mut out = Vec::with_capacity(entry.size() as usize);
                        entry.read_to_end(&mut out).map_err(PackError::from)?;
                        Ok(out)
                    })
                    .await
                    .map_err(|e| PackError::Download(e.to_string()))??;
                let mut f = tokio::fs::File::create(&staging).await?;
                f.write_all(&extracted).await?;
                f.flush().await?;
            }
        }

        // Replace any prior install atomically.
        if target.exists() {
            tokio::fs::remove_file(&target).await?;
        }
        tokio::fs::rename(&staging, &target).await?;

        // Mark executable on Unix; Windows uses extension-based execution.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = tokio::fs::metadata(&target).await?.permissions();
            perms.set_mode(0o755);
            tokio::fs::set_permissions(&target, perms).await?;
        }

        tracing::info!(
            pack = pack_id,
            target = %target.display(),
            bytes = bytes_in,
            "language_pack: github install complete",
        );
        Ok(target.to_string_lossy().into_owned())
    }

    async fn install_npm(&self, pack_id: &'static str, package: &str) -> Result<String, PackError> {
        // Prefer bun (fast + no Node global pollution); fall back to npm.
        // We resolve the absolute path so we can handle `.cmd` shims
        // (npm) vs real `.exe` (bun) differently — `Command::new("npm")`
        // can't actually launch a .cmd file on Windows, but
        // `cmd.exe /C "C:\path\npm.cmd"` does.
        let (cmd_label, binary, args): (&str, std::path::PathBuf, Vec<&str>) = if let Ok(p) =
            which::which("bun")
        {
            // `bun add -g` is the documented form; `bun i -g` is an alias
            // that's been flaky across versions.
            ("bun", p, vec!["add", "-g"])
        } else if let Ok(p) = which::which("npm") {
            ("npm", p, vec!["install", "-g"])
        } else {
            return Err(PackError::DependencyMissing(
                "Neither bun nor npm is on PATH. Install Node.js (https://nodejs.org) or Bun (https://bun.sh) and try again.".into(),
            ));
        };

        let mut full_args = args.clone();
        for piece in package.split_whitespace() {
            full_args.push(piece);
        }

        let is_batch = binary
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| matches!(e.to_ascii_lowercase().as_str(), "cmd" | "bat"))
            .unwrap_or(false);

        let mut cmd = if is_batch {
            let mut c = tokio::process::Command::new("cmd.exe");
            c.arg("/C");
            c.arg(&binary);
            for a in &full_args {
                c.arg(a);
            }
            c
        } else {
            let mut c = tokio::process::Command::new(&binary);
            for a in &full_args {
                c.arg(a);
            }
            c
        };
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.hide_console();

        tracing::info!(
            pack = pack_id,
            installer = cmd_label,
            binary = %binary.display(),
            args = ?full_args,
            "language_pack: spawning install command",
        );

        // Bound the wait. Dropping the future cancels the child via
        // tokio's internal kill-on-drop on `Command::output()`.
        let output = match tokio::time::timeout(NPM_INSTALL_TIMEOUT, cmd.output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(PackError::Command(e.to_string())),
            Err(_) => {
                return Err(PackError::Command(format!(
                    "`{cmd_label} {}` timed out after {}s. Try running the command in a terminal to see what it's waiting on.",
                    full_args.join(" "),
                    NPM_INSTALL_TIMEOUT.as_secs(),
                )));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        tracing::info!(
            pack = pack_id,
            installer = cmd_label,
            exit_code = ?output.status.code(),
            success = output.status.success(),
            stdout_tail = %tail_lines(&stdout, 8),
            stderr_tail = %tail_lines(&stderr, 8),
            "language_pack: install command finished",
        );

        if !output.status.success() {
            let detail = match (stderr.trim().is_empty(), stdout.trim().is_empty()) {
                (false, _) => stderr.trim().to_owned(),
                (true, false) => stdout.trim().to_owned(),
                (true, true) => format!("exit code {:?}", output.status.code()),
            };
            return Err(PackError::Command(format!(
                "`{cmd_label} {}` failed:\n{}",
                full_args.join(" "),
                tail_lines(&detail, 12),
            )));
        }

        // bun / npm install outside our managed dir, so we have to hunt
        // for the binary. Check the well-known per-installer global dirs
        // first — those exist immediately, while PATH may not pick up the
        // change until the shell restarts.
        let stripped = package
            .split_whitespace()
            .next()
            .unwrap_or(package)
            .trim_end_matches(".exe");
        let candidates = well_known_global_candidates(stripped);
        let candidates_dbg: Vec<String> = candidates
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        if let Some(p) = candidates.iter().find(|c| c.exists()).cloned() {
            tracing::info!(pack = pack_id, found_at = %p.display(), "language_pack: install verified");
            return Ok(p.to_string_lossy().into_owned());
        }
        if let Ok(p) = which::which(stripped) {
            tracing::info!(pack = pack_id, found_at = %p.display(), "language_pack: install verified via PATH");
            return Ok(p.to_string_lossy().into_owned());
        }
        tracing::warn!(
            pack = pack_id,
            checked = ?candidates_dbg,
            "language_pack: install reported success but binary not found",
        );
        Err(PackError::Command(format!(
            "`{cmd_label} {}` reported success but the `{stripped}` binary wasn't found in {} known global bin dirs. Open the Oxyris log and search for `language_pack` to see the exact paths checked.",
            full_args.join(" "),
            candidates.len(),
        )))
    }

    /// Install a language pack inside a WSL distro. Spawns `wsl.exe -d
    /// <distro> -- bash -c '<install one-liner>'`. Each pack defines its
    /// own one-liner: rust-analyzer downloads the gnu Linux release;
    /// TS/PHP packs `npm i -g` (npm must already be on the distro PATH).
    /// Idempotent — re-running over an existing install just overwrites.
    ///
    /// Emits `language_pack:status` with id `wsl/<distro>/<pack>` so the
    /// UI can show progress separately from Windows-side installs.
    pub async fn install_in_wsl(
        self: &Arc<Self>,
        distro: &str,
        id: &str,
    ) -> Result<String, PackError> {
        let pack = find(id).ok_or_else(|| PackError::Unknown(id.to_owned()))?;
        let one_liner = wsl_install_one_liner(pack)?;

        let event_id: &'static str = Box::leak(format!("wsl/{distro}/{id}").into_boxed_str());
        self.emit(PackProgressEvent::Started { id: event_id });
        tracing::info!(pack = id, distro, "language_pack: starting wsl install");

        // Stream the install script over stdin to `bash -l` instead of
        // passing it as an arg. Login shell so user-PATH (nvm, asdf, …)
        // is loaded; stdin so wsl.exe never gets to mangle the script.
        let mut cmd = tokio::process::Command::new("wsl.exe");
        cmd.arg("-d")
            .arg(distro)
            .arg("--")
            .arg("bash")
            .arg("-l")
            .arg("-s");
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.hide_console();

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("wsl spawn failed: {e}");
                self.emit(PackProgressEvent::Failed {
                    id: event_id,
                    message: msg.clone(),
                });
                return Err(PackError::Command(msg));
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = stdin.write_all(one_liner.as_bytes()).await {
                let msg = format!("write install script to wsl stdin: {e}");
                self.emit(PackProgressEvent::Failed {
                    id: event_id,
                    message: msg.clone(),
                });
                return Err(PackError::Command(msg));
            }
            let _ = stdin.shutdown().await;
        }

        let result = tokio::time::timeout(NPM_INSTALL_TIMEOUT, child.wait_with_output()).await;
        match result {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                tracing::info!(
                    pack = id,
                    distro,
                    exit_code = ?out.status.code(),
                    success = out.status.success(),
                    stdout_tail = %tail_lines(&stdout, 8),
                    stderr_tail = %tail_lines(&stderr, 8),
                    "language_pack: wsl install finished",
                );
                if out.status.success() {
                    // Resolved binary path is the last non-empty stdout line —
                    // matches the `echo \"$resolved\"` in `npm_install_one_liner`.
                    let path = stdout
                        .lines()
                        .map(str::trim)
                        .rfind(|l| !l.is_empty())
                        .map(|s| s.to_owned())
                        .unwrap_or_else(|| format!("~/.local/bin/{}", pack.binary_name));
                    self.record_wsl_install(pack.id, distro, &path).await;
                    self.emit(PackProgressEvent::Done {
                        id: event_id,
                        path: path.clone(),
                    });
                    Ok(path)
                } else {
                    let detail = if stderr.trim().is_empty() {
                        stdout.trim().to_owned()
                    } else {
                        stderr.trim().to_owned()
                    };
                    let msg = format!(
                        "wsl install of `{}` in `{distro}` failed:\n{}",
                        pack.id,
                        tail_lines(&detail, 12)
                    );
                    self.emit(PackProgressEvent::Failed {
                        id: event_id,
                        message: msg.clone(),
                    });
                    Err(PackError::Command(msg))
                }
            }
            Ok(Err(e)) => {
                let msg = format!("wsl spawn failed: {e}");
                self.emit(PackProgressEvent::Failed {
                    id: event_id,
                    message: msg.clone(),
                });
                Err(PackError::Command(msg))
            }
            Err(_) => {
                let msg = format!(
                    "wsl install of `{id}` in `{distro}` timed out after {}s",
                    NPM_INSTALL_TIMEOUT.as_secs()
                );
                self.emit(PackProgressEvent::Failed {
                    id: event_id,
                    message: msg.clone(),
                });
                Err(PackError::Command(msg))
            }
        }
    }

    fn emit(&self, event: PackProgressEvent) {
        if let Err(e) = self.app.emit("language_pack:status", &event) {
            tracing::debug!(error = %e, "language_pack:status emit failed");
        }
    }
}

/// Per-pack bash one-liner that installs the LSP into the WSL distro.
///
/// - Rust: pulls the official `linux-gnu` rust-analyzer release into
///   `~/.local/bin/rust-analyzer` (works for both glibc and musl distros
///   that ship the standard runtime).
/// - TS/PHP: `npm i -g` — the user must have npm available in the
///   distro. We don't try to install npm itself; that's a distro-package
///   decision (apt / dnf / apk / etc.).
fn wsl_install_one_liner(pack: &Pack) -> Result<String, PackError> {
    use oxyris_lsp::LspLanguage;
    let cmd = match pack.lsp_language {
        LspLanguage::Rust => {
            "set -e; \
             mkdir -p ~/.local/bin; \
             tmp=$(mktemp); \
             curl -fsSL -o \"$tmp.gz\" https://github.com/rust-lang/rust-analyzer/releases/latest/download/rust-analyzer-x86_64-unknown-linux-gnu.gz; \
             gunzip -f \"$tmp.gz\"; \
             mv \"$tmp\" ~/.local/bin/rust-analyzer; \
             chmod +x ~/.local/bin/rust-analyzer; \
             ~/.local/bin/rust-analyzer --version"
                .to_owned()
        }
        LspLanguage::TypeScriptJavaScript => npm_install_one_liner(
            "typescript-language-server typescript",
            "typescript-language-server",
        ),
        LspLanguage::Php => {
            npm_install_one_liner("intelephense", "intelephense")
        }
    };
    Ok(cmd)
}

/// Bash one-liner that installs an npm package globally inside a WSL
/// distro. Two safety nets:
///
/// 1. **Strip `/mnt/*` from PATH up-front.** When WSL→Win32 interop is on
///    and node isn't installed natively, every Windows-side `node`/`npm`
///    leaks into the distro's PATH at `/mnt/c/Program Files/nodejs/...`.
///    Running through that interop shim lands the package under
///    `%APPDATA%\npm` on Windows — *not* in the distro — and the LSP
///    server can't be invoked from inside WSL. Pruning the `/mnt/`
///    entries first guarantees only a real distro npm can satisfy
///    `command -v npm`. Pure detection (matching `/mnt/*`) wasn't
///    reliable: `command -v` output format varies (alias resolution,
///    WSLInterop registration, …) and one shape slipped past pattern
///    matching, ran the Windows shim, and failed at parse time on the
///    `Program Files` space.
/// 2. **Auto-install via passwordless apt** when npm is genuinely
///    absent and `sudo -n` works; otherwise print distro-specific
///    install instructions.
fn npm_install_one_liner(packages: &str, verify_bin: &str) -> String {
    // Bash receives this on stdin from `bash -l -s`. Login mode reads
    // `~/.profile` / `~/.bash_profile`, but **non-interactive** bash skips
    // most `~/.bashrc` content (typical guard: `[ -z \"$PS1\" ] && return`),
    // so version managers like nvm/fnm/asdf — which install themselves into
    // `.bashrc` — are not active here. Bootstrap them explicitly, then
    // strip `/mnt/*` to refuse the Windows interop shim, then verify a
    // distro-native npm is reachable.
    format!(
        r#"set -e
# ---- Make user-installed node toolchains visible to non-interactive bash. ----
# nvm
if [ -z "$NVM_DIR" ] && [ -d "$HOME/.nvm" ]; then export NVM_DIR="$HOME/.nvm"; fi
if [ -s "$NVM_DIR/nvm.sh" ]; then
  # shellcheck disable=SC1090
  . "$NVM_DIR/nvm.sh" >/dev/null 2>&1 || true
fi
# fnm
if command -v fnm >/dev/null 2>&1; then
  eval "$(fnm env --use-on-cd 2>/dev/null)" >/dev/null 2>&1 || true
fi
# asdf
if [ -s "$HOME/.asdf/asdf.sh" ]; then
  # shellcheck disable=SC1091
  . "$HOME/.asdf/asdf.sh" >/dev/null 2>&1 || true
fi
# Common bin dirs that ship a distro-native node/npm.
for d in "$HOME/.local/bin" "$HOME/.npm-global/bin" "$HOME/.bun/bin" \
         /usr/local/bin /usr/bin /opt/nodejs/bin; do
  if [ -d "$d" ]; then
    case ":$PATH:" in *":$d:"*) ;; *) PATH="$d:$PATH" ;; esac
  fi
done
export PATH

# ---- Strip Windows interop entries — guarantees no /mnt/c/.../npm wins. ----
clean_path=$(printf '%s' "$PATH" | tr ':' '\n' | grep -v '^/mnt/' | grep -v '^$' | paste -sd: -)
export PATH="$clean_path"

# ---- Verify npm reachable; auto-install via apt only if sudo -n works. ----
if ! command -v npm >/dev/null 2>&1; then
  echo 'npm not found in distro PATH (after stripping Windows interop entries).' >&2
  echo 'Trying passwordless apt install...' >&2
  if command -v apt-get >/dev/null 2>&1 && sudo -n true >/dev/null 2>&1; then
    sudo -n apt-get update -y >&2 && sudo -n apt-get install -y nodejs npm >&2 || true
  fi
fi
if ! command -v npm >/dev/null 2>&1; then
  echo '' >&2
  echo 'ERROR: npm is not installed natively in this WSL distro.' >&2
  echo 'Oxyris refuses to use the Windows npm interop shim — packages installed' >&2
  echo 'through it land in %APPDATA%\npm on Windows, not inside the distro, and' >&2
  echo 'the LSP server would not be reachable from WSL.' >&2
  echo '' >&2
  echo 'Searched PATH:' >&2
  echo "  $PATH" >&2
  echo '' >&2
  echo 'If you have npm installed via nvm/fnm/asdf and it is only loaded in' >&2
  echo 'interactive shells, run `which npm` in your distro terminal and add' >&2
  echo "its directory to your shell's non-interactive PATH (e.g. ~/.profile)." >&2
  echo '' >&2
  echo 'Otherwise install Node natively, then retry:' >&2
  echo '  Debian/Ubuntu:   sudo apt update && sudo apt install -y nodejs npm' >&2
  echo '  Alpine:          sudo apk add --no-cache nodejs npm' >&2
  echo '  Fedora/RHEL:     sudo dnf install -y nodejs npm' >&2
  echo '  Arch:            sudo pacman -S --noconfirm nodejs npm' >&2
  exit 1
fi

# ---- Install + report final binary path. ----
npm install -g {packages} >&2
resolved=$(command -v {verify_bin} 2>/dev/null || true)
if [ -z "$resolved" ]; then
  echo "{verify_bin} not on PATH after install" >&2
  exit 1
fi
case "$resolved" in
  /mnt/*) echo "{verify_bin} ended up at $resolved (Windows side) — install rejected." >&2; exit 1 ;;
esac
echo "$resolved"
"#
    )
}

fn install_method_label(m: &InstallMethod) -> &'static str {
    match m {
        InstallMethod::GithubRelease { .. } => "github_release",
        InstallMethod::NpmGlobal { .. } => "npm_global",
        InstallMethod::Manual { .. } => "manual",
    }
}

/// Resolve a binary outside the managed dir. Checks bun/npm well-known
/// global directories explicitly so a freshly installed package is found
/// without needing the user to restart their shell, then falls back to
/// `which::which` for anything else on PATH.
///
/// Validates rustup proxies (binaries inside `~/.cargo/bin/`): rustup ships
/// a shim that exits with `Unknown binary` if the corresponding component
/// isn't installed. We can't use it as-is, so we resolve through
/// `rustup which <name>` which fails fast when the component is missing.
fn find_external_binary(name: &str) -> Option<PathBuf> {
    let stripped = name.trim_end_matches(".exe");
    for candidate in well_known_global_candidates(stripped) {
        if candidate.exists() {
            return validate_external(candidate, stripped);
        }
    }
    which::which(stripped)
        .ok()
        .and_then(|p| validate_external(p, stripped))
}

/// Returns Some(path) if the binary is usable, None if it's a broken shim
/// (e.g. rustup proxy with no component installed).
fn validate_external(path: PathBuf, name: &str) -> Option<PathBuf> {
    if is_rustup_proxy(&path) {
        return resolve_via_rustup(name);
    }
    Some(path)
}

fn is_rustup_proxy(path: &Path) -> bool {
    let cargo_bin = match std::env::var_os(
        #[cfg(target_os = "windows")]
        "USERPROFILE",
        #[cfg(not(target_os = "windows"))]
        "HOME",
    ) {
        Some(h) => PathBuf::from(h).join(".cargo").join("bin"),
        None => return false,
    };
    path.parent()
        .map(|p| p == cargo_bin.as_path())
        .unwrap_or(false)
}

/// Ask rustup for the real binary path. Succeeds only if the corresponding
/// component is installed in the active toolchain.
fn resolve_via_rustup(name: &str) -> Option<PathBuf> {
    use oxyris_procutil::HideConsole;
    let out = std::process::Command::new("rustup")
        .args(["which", name])
        .hide_console()
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(path))
}

#[cfg(target_os = "windows")]
fn well_known_global_candidates(name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let home = PathBuf::from(home);
        // bun's per-user global bin (Windows installs ship as .exe shims).
        let bun_bin = home.join(".bun").join("bin");
        out.push(bun_bin.join(format!("{name}.exe")));
        out.push(bun_bin.join(name));
        // Cargo / rustup binaries — Tauri's process PATH may miss this if
        // the user only has it via rustup's shell init. Check explicitly.
        let cargo_bin = home.join(".cargo").join("bin");
        out.push(cargo_bin.join(format!("{name}.exe")));
        out.push(cargo_bin.join(name));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let npm_bin = PathBuf::from(appdata).join("npm");
        // npm on Windows installs PowerShell shims with `.cmd` (and a `.ps1`).
        out.push(npm_bin.join(format!("{name}.cmd")));
        out.push(npm_bin.join(format!("{name}.exe")));
        out.push(npm_bin.join(name));
    }
    out
}

#[cfg(not(target_os = "windows"))]
fn well_known_global_candidates(name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        out.push(home.join(".bun").join("bin").join(name));
        out.push(home.join(".cargo").join("bin").join(name));
    }
    out.push(PathBuf::from("/usr/local/bin").join(name));
    out.push(PathBuf::from("/usr/bin").join(name));
    out
}

/// Take the last `n` lines of a multi-line string. Useful when surfacing
/// install command output: the tail almost always has the actual error.
fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= n {
        return s.to_owned();
    }
    let mut out = String::from("…\n");
    for line in &lines[lines.len() - n..] {
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_owned()
}

fn wsl_installs_path(data_dir: &Path) -> PathBuf {
    data_dir.join("language_packs_wsl.json")
}

fn load_wsl_installs(data_dir: &Path) -> std::collections::HashMap<String, Vec<WslInstallInfo>> {
    let path = wsl_installs_path(data_dir);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return std::collections::HashMap::new(),
    };
    match serde_json::from_slice(&bytes) {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "language_pack: failed to parse wsl_installs file; starting empty");
            std::collections::HashMap::new()
        }
    }
}

fn persist_wsl_installs(
    data_dir: &Path,
    map: &std::collections::HashMap<String, Vec<WslInstallInfo>>,
) -> std::io::Result<()> {
    if let Err(e) = std::fs::create_dir_all(data_dir)
        && e.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(e);
    }
    let json = serde_json::to_vec_pretty(map).map_err(std::io::Error::other)?;
    std::fs::write(wsl_installs_path(data_dir), json)
}
