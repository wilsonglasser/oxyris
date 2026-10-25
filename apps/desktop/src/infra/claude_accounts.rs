//! Claude Code account registry and one-click switching, per environment.
//!
//! Windows (`Environment::Local`) and each WSL distro keep **separate** pools:
//! the CLI in each environment owns its own `~/.claude` and its own token
//! family, so an account registered for Windows is never written into a
//! distro or vice versa.
//!
//! **Slots.** Every registered account lives in its own directory laid out
//! like a `CLAUDE_CONFIG_DIR` (`.credentials.json` + `.claude.json`):
//!
//! - Windows: `<data_dir>/claude-accounts/<id>/`
//! - WSL: `~/.oxyris/claude-accounts/<id>/` inside the distro
//!
//! That layout lets us hand a slot straight to the CLI — `claude auth login`
//! for a new reserve, `claude -p` for a token refresh — without touching the
//! live login. Tokens there are plaintext, exactly like the CLI's own
//! `~/.claude/.credentials.json`; on Linux they are written `0600`.
//!
//! **Switching** copies only the `claudeAiOauth` / `oauthAccount` subtrees of
//! the slot over the live files (see `oxyris_claude::accounts`). Refresh
//! tokens rotate on every refresh, so before overwriting the live login we
//! snapshot it back into its own slot (registering it if it wasn't yet) —
//! otherwise the stored copy of the account being switched away from would be
//! stale and dead.
//!
//! **Routing.** Windows uses `std::fs`; WSL goes through the agent
//! (`fs.read`/`fs.write`/`fs.rename`/`fs.delete`) — never `\\wsl.localhost`.
//! The two CLI spawns (login console, headless refresh) use `wsl.exe` the same
//! way `provider_discovery` and the pure-mode PTY already do for one-shot /
//! interactive `claude` runs. Nothing here ever boots a distro: every public
//! entry point fails with [`AccountsError::WslOffline`] when it isn't already
//! running (see [`ClaudeAccounts::is_online`]).
//!
//! Accounts are environment configuration, not an event-sourced aggregate:
//! the registry is a JSON file next to `keybindings.json`, and credentials
//! must never land in the append-only event log anyway.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use oxyris_claude::accounts::{
    self as docs, AccountDocError, AccountIdentity, OAuthCredentials, Plan,
};
use oxyris_core::Environment;
use oxyris_ipc::ops::{FsDeleteArgs, FsReadArgs, FsRenameArgs, FsWriteArgs, op_name};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::infra::agent_pool::{AgentError, AgentPool};

const CREDENTIALS_FILE: &str = ".credentials.json";
const CONFIG_FILE: &str = ".claude.json";
const REGISTRY_FILE: &str = "claude-accounts.json";
const SLOTS_DIR: &str = "claude-accounts";
/// `.claude.json` grows with history; read well past the agent's 1 MiB
/// default and refuse to write back anything that came back truncated.
const MAX_DOC_BYTES: u64 = 64 * 1024 * 1024;
const PRIVATE_MODE: u32 = 0o600;
const REFRESH_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Error)]
pub enum AccountsError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("agent: {0}")]
    Agent(#[from] AgentError),
    #[error("{0}")]
    Doc(#[from] AccountDocError),
    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("account not found")]
    NotFound,
    #[error("no Claude login found in this environment")]
    NoLiveLogin,
    #[error("{0} came back truncated; refusing to rewrite it")]
    Truncated(String),
    #[error("switch could not be verified and was rolled back: {0}")]
    VerifyFailed(String),
    #[error("{0}")]
    Spawn(String),
    /// The distro isn't running. We never boot one for account work — the
    /// user starts it (terminal, WSL project) and retries.
    #[error("WSL distro {0} is not running")]
    WslOffline(String),
}

/// One registered account. Persisted in the registry; carries no tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountRecord {
    pub id: String,
    pub environment: Environment,
    pub identity: AccountIdentity,
    pub plan: Plan,
    pub added_at_ms: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    accounts: Vec<AccountRecord>,
}

/// Who is logged in right now in an environment.
#[derive(Debug, Clone, Serialize)]
pub struct LiveAccount {
    pub identity: AccountIdentity,
    pub plan: Plan,
}

/// Stable key for an environment in maps and events: `local` / `wsl:<distro>`.
pub fn env_key(env: &Environment) -> String {
    match env {
        Environment::Local => "local".to_owned(),
        Environment::Wsl { distro } => format!("wsl:{distro}"),
    }
}

/// Where usage credentials come from for one account.
#[derive(Debug, Clone)]
pub enum CredSource {
    /// The environment's live `~/.claude`.
    Live,
    /// A registered slot directory.
    Slot(String),
}

pub struct ClaudeAccounts {
    data_dir: PathBuf,
    agent_pool: Arc<AgentPool>,
    /// Serializes registry + live-file mutations (switch, save, remove).
    lock: Mutex<()>,
    wsl_homes: Mutex<HashMap<String, String>>,
    /// Tests point the Windows live login at a temp dir (the real resolution
    /// reads process-global env vars).
    #[cfg(test)]
    local_live_dir: Option<PathBuf>,
}

/// File access routed by environment.
enum EnvFs<'a> {
    Local,
    Wsl {
        distro: &'a str,
        pool: &'a AgentPool,
    },
}

impl EnvFs<'_> {
    async fn read(&self, path: &str) -> Result<Option<String>, AccountsError> {
        match self {
            EnvFs::Local => match std::fs::read_to_string(path) {
                Ok(s) => Ok(Some(s)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            },
            EnvFs::Wsl { distro, pool } => {
                let args = serde_json::to_value(FsReadArgs {
                    path: path.to_owned(),
                    max_bytes: Some(MAX_DOC_BYTES),
                })?;
                match pool.call(distro, op_name::FS_READ, args).await {
                    Ok(v) => {
                        if v.get("truncated").and_then(|t| t.as_bool()) == Some(true) {
                            return Err(AccountsError::Truncated(path.to_owned()));
                        }
                        Ok(v.get("content").and_then(|c| c.as_str()).map(str::to_owned))
                    }
                    Err(AgentError::Remote { code, .. }) if code == "not_found" => Ok(None),
                    Err(e) => Err(e.into()),
                }
            }
        }
    }

    /// Write via sibling temp + rename so the CLI (which reads these files
    /// constantly) never sees a half-written document.
    async fn write_private(&self, path: &str, contents: &str) -> Result<(), AccountsError> {
        let tmp = format!("{path}.oxyris-{}.tmp", uuid::Uuid::now_v7().simple());
        match self {
            EnvFs::Local => {
                if let Some(parent) = Path::new(path).parent() {
                    std::fs::create_dir_all(parent)?;
                }
                write_local_private(Path::new(&tmp), contents)?;
                if let Err(e) = std::fs::rename(&tmp, path) {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e.into());
                }
                Ok(())
            }
            EnvFs::Wsl { distro, pool } => {
                let args = serde_json::to_value(FsWriteArgs {
                    path: tmp.clone(),
                    contents: contents.to_owned(),
                    mode: Some(PRIVATE_MODE),
                })?;
                pool.call(distro, op_name::FS_WRITE, args).await?;
                let args = serde_json::to_value(FsRenameArgs {
                    from: tmp.clone(),
                    to: path.to_owned(),
                })?;
                if let Err(e) = pool.call(distro, op_name::FS_RENAME, args).await {
                    let _ = self.remove(&tmp, false).await;
                    return Err(e.into());
                }
                Ok(())
            }
        }
    }

    async fn remove(&self, path: &str, recursive: bool) -> Result<(), AccountsError> {
        match self {
            EnvFs::Local => {
                let p = Path::new(path);
                let res = if recursive {
                    std::fs::remove_dir_all(p)
                } else {
                    std::fs::remove_file(p)
                };
                match res {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                    _ => Ok(()),
                }
            }
            EnvFs::Wsl { distro, pool } => {
                let args = serde_json::to_value(FsDeleteArgs {
                    path: path.to_owned(),
                    recursive,
                })?;
                match pool.call(distro, op_name::FS_DELETE, args).await {
                    Err(AgentError::Remote { code, .. }) if code == "not_found" => Ok(()),
                    Err(e) => Err(e.into()),
                    Ok(_) => Ok(()),
                }
            }
        }
    }
}

fn write_local_private(path: &Path, contents: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(PRIVATE_MODE)
            .open(path)?;
        f.write_all(contents.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
}

/// Paths of one config directory (live or slot) in environment-native form.
#[derive(Debug, Clone)]
struct DocPaths {
    credentials: String,
    config: String,
}

fn join(dir: &str, leaf: &str, wsl: bool) -> String {
    if wsl {
        format!("{}/{leaf}", dir.trim_end_matches('/'))
    } else {
        Path::new(dir).join(leaf).to_string_lossy().into_owned()
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl ClaudeAccounts {
    pub fn new(data_dir: PathBuf, agent_pool: Arc<AgentPool>) -> Self {
        Self {
            data_dir,
            agent_pool,
            lock: Mutex::new(()),
            wsl_homes: Mutex::new(HashMap::new()),
            #[cfg(test)]
            local_live_dir: None,
        }
    }

    /// Windows is always online; a distro only when it's already running.
    pub async fn is_online(&self, env: &Environment) -> bool {
        match env {
            Environment::Local => true,
            Environment::Wsl { distro } => {
                let distro = distro.clone();
                tokio::task::spawn_blocking(move || {
                    crate::infra::environments::wsl_distro_running(&distro)
                })
                .await
                .unwrap_or(false)
            }
        }
    }

    async fn require_online(&self, env: &Environment) -> Result<(), AccountsError> {
        match env {
            Environment::Wsl { distro } if !self.is_online(env).await => {
                Err(AccountsError::WslOffline(distro.clone()))
            }
            _ => Ok(()),
        }
    }

    fn fs<'a>(&'a self, env: &'a Environment) -> EnvFs<'a> {
        match env {
            Environment::Local => EnvFs::Local,
            Environment::Wsl { distro } => EnvFs::Wsl {
                distro,
                pool: &self.agent_pool,
            },
        }
    }

    async fn wsl_home(&self, distro: &str) -> Result<String, AccountsError> {
        if let Some(h) = self.wsl_homes.lock().await.get(distro) {
            return Ok(h.clone());
        }
        let info = self
            .agent_pool
            .call(distro, op_name::SYSTEM_INFO, serde_json::json!({}))
            .await?;
        let home = info
            .get("home")
            .and_then(|h| h.as_str())
            .filter(|h| !h.is_empty())
            .ok_or_else(|| AccountsError::Spawn(format!("no $HOME reported by {distro}")))?
            .to_owned();
        self.wsl_homes
            .lock()
            .await
            .insert(distro.to_owned(), home.clone());
        Ok(home)
    }

    /// The live config dir's documents. Windows honours `CLAUDE_CONFIG_DIR`
    /// (and the legacy `.config.json` name) the way the CLI resolves it; for
    /// WSL we can't see the user's shell env, so it's the default `~/.claude`.
    async fn live_paths(&self, env: &Environment) -> Result<DocPaths, AccountsError> {
        match env {
            #[cfg(test)]
            Environment::Local if self.local_live_dir.is_some() => {
                let dir = self.local_live_dir.as_ref().expect("checked");
                Ok(DocPaths {
                    credentials: dir.join(CREDENTIALS_FILE).to_string_lossy().into_owned(),
                    config: dir.join(CONFIG_FILE).to_string_lossy().into_owned(),
                })
            }
            Environment::Local => {
                let home = oxyris_procutil::home_dir()
                    .ok_or_else(|| AccountsError::Spawn("home directory not found".into()))?;
                let explicit = std::env::var_os("CLAUDE_CONFIG_DIR")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from);
                let dir = explicit.clone().unwrap_or_else(|| home.join(".claude"));
                let legacy = dir.join(".config.json");
                let config = if legacy.exists() {
                    legacy
                } else if explicit.is_some() {
                    dir.join(CONFIG_FILE)
                } else {
                    home.join(CONFIG_FILE)
                };
                Ok(DocPaths {
                    credentials: dir.join(CREDENTIALS_FILE).to_string_lossy().into_owned(),
                    config: config.to_string_lossy().into_owned(),
                })
            }
            Environment::Wsl { distro } => {
                let home = self.wsl_home(distro).await?;
                Ok(DocPaths {
                    credentials: join(&format!("{home}/.claude"), CREDENTIALS_FILE, true),
                    config: join(&home, CONFIG_FILE, true),
                })
            }
        }
    }

    /// Environment-native directory of a slot.
    pub async fn slot_dir(&self, env: &Environment, id: &str) -> Result<String, AccountsError> {
        match env {
            Environment::Local => Ok(self
                .data_dir
                .join(SLOTS_DIR)
                .join(id)
                .to_string_lossy()
                .into_owned()),
            Environment::Wsl { distro } => {
                let home = self.wsl_home(distro).await?;
                Ok(format!("{home}/.oxyris/{SLOTS_DIR}/{id}"))
            }
        }
    }

    async fn slot_paths(&self, env: &Environment, id: &str) -> Result<DocPaths, AccountsError> {
        let dir = self.slot_dir(env, id).await?;
        let wsl = matches!(env, Environment::Wsl { .. });
        Ok(DocPaths {
            credentials: join(&dir, CREDENTIALS_FILE, wsl),
            config: join(&dir, CONFIG_FILE, wsl),
        })
    }

    fn read_registry(&self) -> Registry {
        std::fs::read_to_string(self.data_dir.join(REGISTRY_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn write_registry(&self, reg: &Registry) -> Result<(), AccountsError> {
        let path = self.data_dir.join(REGISTRY_FILE);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(reg)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Registered accounts for one environment.
    pub fn records(&self, env: &Environment) -> Vec<AccountRecord> {
        self.read_registry()
            .accounts
            .into_iter()
            .filter(|a| &a.environment == env)
            .collect()
    }

    /// Environments that have at least one registered account.
    pub fn registered_environments(&self) -> Vec<Environment> {
        let mut out: Vec<Environment> = Vec::new();
        for a in self.read_registry().accounts {
            if !out.contains(&a.environment) {
                out.push(a.environment);
            }
        }
        out
    }

    /// Identity + plan of the live login, `Ok(None)` when nobody is logged in.
    pub async fn live(&self, env: &Environment) -> Result<Option<LiveAccount>, AccountsError> {
        self.require_online(env).await?;
        let paths = self.live_paths(env).await?;
        let fs = self.fs(env);
        let (Some(creds), Some(config)) = (
            fs.read(&paths.credentials).await?,
            fs.read(&paths.config).await?,
        ) else {
            return Ok(None);
        };
        let identity = match docs::parse_identity(&config) {
            Ok(id) => id,
            Err(AccountDocError::Missing(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let plan = match docs::parse_credentials(&creds) {
            Ok(c) => c.plan(),
            Err(AccountDocError::Missing(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(Some(LiveAccount { identity, plan }))
    }

    /// OAuth credentials for a usage request.
    pub async fn credentials(
        &self,
        env: &Environment,
        source: &CredSource,
    ) -> Result<OAuthCredentials, AccountsError> {
        self.require_online(env).await?;
        let paths = match source {
            CredSource::Live => self.live_paths(env).await?,
            CredSource::Slot(id) => self.slot_paths(env, id).await?,
        };
        let creds = self
            .fs(env)
            .read(&paths.credentials)
            .await?
            .ok_or(AccountsError::NoLiveLogin)?;
        Ok(docs::parse_credentials(&creds)?)
    }

    /// Copy the live login into `id`'s slot (minimal docs: just the account
    /// subtrees — no MCP tokens, no history).
    async fn snapshot_live_into(
        &self,
        env: &Environment,
        id: &str,
        live_creds: &str,
        live_config: &str,
    ) -> Result<(), AccountsError> {
        let slot = self.slot_paths(env, id).await?;
        let fs = self.fs(env);
        fs.write_private(
            &slot.credentials,
            &docs::swap_oauth_credentials(None, live_creds)?,
        )
        .await?;
        fs.write_private(&slot.config, &docs::swap_oauth_account(None, live_config)?)
            .await?;
        Ok(())
    }

    /// Register (or update) the live login as a reserve slot.
    pub async fn save_live(&self, env: &Environment) -> Result<AccountRecord, AccountsError> {
        self.require_online(env).await?;
        let _guard = self.lock.lock().await;
        self.save_live_locked(env).await
    }

    async fn save_live_locked(&self, env: &Environment) -> Result<AccountRecord, AccountsError> {
        let paths = self.live_paths(env).await?;
        let fs = self.fs(env);
        let creds = fs
            .read(&paths.credentials)
            .await?
            .ok_or(AccountsError::NoLiveLogin)?;
        let config = fs
            .read(&paths.config)
            .await?
            .ok_or(AccountsError::NoLiveLogin)?;
        let identity = docs::parse_identity(&config)?;
        let plan = docs::parse_credentials(&creds)?.plan();

        let mut reg = self.read_registry();
        let existing = reg
            .accounts
            .iter()
            .position(|a| &a.environment == env && a.identity.same_account(&identity));
        let id = existing
            .map(|i| reg.accounts[i].id.clone())
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        self.snapshot_live_into(env, &id, &creds, &config).await?;

        let record = AccountRecord {
            id,
            environment: env.clone(),
            identity,
            plan,
            added_at_ms: existing
                .map(|i| reg.accounts[i].added_at_ms)
                .unwrap_or_else(now_ms),
        };
        match existing {
            Some(i) => reg.accounts[i] = record.clone(),
            None => reg.accounts.push(record.clone()),
        }
        self.write_registry(&reg)?;
        Ok(record)
    }

    /// Make `id` the live login of `env`.
    pub async fn activate(&self, env: &Environment, id: &str) -> Result<(), AccountsError> {
        self.require_online(env).await?;
        let _guard = self.lock.lock().await;
        let reg = self.read_registry();
        let target = reg
            .accounts
            .iter()
            .find(|a| &a.environment == env && a.id == id)
            .cloned()
            .ok_or(AccountsError::NotFound)?;

        let fs = self.fs(env);
        let slot = self.slot_paths(env, id).await?;
        let slot_creds = fs
            .read(&slot.credentials)
            .await?
            .ok_or(AccountsError::NotFound)?;
        let slot_config = fs
            .read(&slot.config)
            .await?
            .ok_or(AccountsError::NotFound)?;
        // Fail before touching anything if the slot is unusable.
        docs::parse_credentials(&slot_creds)?;
        docs::parse_identity(&slot_config)?;

        let live = self.live_paths(env).await?;
        let live_creds = fs.read(&live.credentials).await?;
        let live_config = fs.read(&live.config).await?;
        let live_identity = live_config
            .as_deref()
            .and_then(|c| docs::parse_identity(c).ok());

        if let Some(current) = &live_identity {
            if current.same_account(&target.identity) {
                return Ok(());
            }
            // Preserve the outgoing login (its refresh token has rotated
            // since it was last stored) — registering it if it's new.
            if live_creds
                .as_deref()
                .is_some_and(|c| docs::parse_credentials(c).is_ok())
            {
                self.save_live_locked(env).await?;
            }
        }

        let new_creds = docs::swap_oauth_credentials(live_creds.as_deref(), &slot_creds)?;
        let new_config = docs::swap_oauth_account(live_config.as_deref(), &slot_config)?;

        fs.write_private(&live.credentials, &new_creds).await?;
        if let Err(e) = fs.write_private(&live.config, &new_config).await {
            self.restore(&fs, &live.credentials, live_creds.as_deref())
                .await;
            return Err(e);
        }

        let verified = match fs.read(&live.config).await {
            Ok(Some(c)) => docs::parse_identity(&c)
                .map(|id| id.same_account(&target.identity))
                .unwrap_or(false),
            _ => false,
        };
        if !verified {
            self.restore(&fs, &live.credentials, live_creds.as_deref())
                .await;
            self.restore(&fs, &live.config, live_config.as_deref())
                .await;
            return Err(AccountsError::VerifyFailed(target.identity.email));
        }
        tracing::info!(env = %env_key(env), account = %target.identity.email, "claude account switched");
        Ok(())
    }

    async fn restore(&self, fs: &EnvFs<'_>, path: &str, original: Option<&str>) {
        let res = match original {
            Some(text) => fs.write_private(path, text).await,
            None => fs.remove(path, false).await,
        };
        if let Err(e) = res {
            tracing::error!(%path, "restore after failed claude account switch: {e}");
        }
    }

    /// Unregister `id` and delete its slot. The live login is never touched,
    /// even when `id` is the active account.
    pub async fn remove(&self, env: &Environment, id: &str) -> Result<(), AccountsError> {
        self.require_online(env).await?;
        let _guard = self.lock.lock().await;
        let mut reg = self.read_registry();
        let before = reg.accounts.len();
        reg.accounts
            .retain(|a| !(&a.environment == env && a.id == id));
        if reg.accounts.len() == before {
            return Err(AccountsError::NotFound);
        }
        let dir = self.slot_dir(env, id).await?;
        self.fs(env).remove(&dir, true).await?;
        self.write_registry(&reg)?;
        Ok(())
    }

    /// Run `claude auth login` in a visible console against a fresh slot dir,
    /// wait for it to exit, then register whatever account it logged in.
    /// Returns `Ok(None)` when the console closed without a login.
    pub async fn login_new(
        &self,
        env: &Environment,
    ) -> Result<Option<AccountRecord>, AccountsError> {
        self.require_online(env).await?;
        let id = uuid::Uuid::now_v7().to_string();
        let dir = self.slot_dir(env, &id).await?;
        let fs = self.fs(env);
        // Materialize the dir up front; the agent's write creates parents.
        let marker = join(&dir, ".oxyris-slot", matches!(env, Environment::Wsl { .. }));
        fs.write_private(&marker, "").await?;

        let status = spawn_login_console(env, &dir).await;
        let outcome = match status {
            Ok(()) => self.adopt_login_slot(env, &id).await,
            Err(e) => Err(e),
        };
        if !matches!(outcome, Ok(Some(ref r)) if r.id == id) {
            let _ = fs.remove(&dir, true).await;
        }
        outcome
    }

    async fn adopt_login_slot(
        &self,
        env: &Environment,
        id: &str,
    ) -> Result<Option<AccountRecord>, AccountsError> {
        let _guard = self.lock.lock().await;
        let fs = self.fs(env);
        let slot = self.slot_paths(env, id).await?;
        let (Some(creds), Some(config)) = (
            fs.read(&slot.credentials).await?,
            fs.read(&slot.config).await?,
        ) else {
            return Ok(None);
        };
        let Ok(credentials) = docs::parse_credentials(&creds) else {
            return Ok(None);
        };
        let identity = docs::parse_identity(&config)?;

        let mut reg = self.read_registry();
        if let Some(i) = reg
            .accounts
            .iter()
            .position(|a| &a.environment == env && a.identity.same_account(&identity))
        {
            // Re-login of an account we already have: refresh that slot with
            // the new token pair instead of creating a duplicate.
            let existing = reg.accounts[i].id.clone();
            let target = self.slot_paths(env, &existing).await?;
            fs.write_private(&target.credentials, &creds).await?;
            fs.write_private(&target.config, &config).await?;
            reg.accounts[i].plan = credentials.plan();
            reg.accounts[i].identity = identity;
            let record = reg.accounts[i].clone();
            self.write_registry(&reg)?;
            return Ok(Some(record));
        }

        let record = AccountRecord {
            id: id.to_owned(),
            environment: env.clone(),
            identity,
            plan: credentials.plan(),
            added_at_ms: now_ms(),
        };
        reg.accounts.push(record.clone());
        self.write_registry(&reg)?;
        Ok(Some(record))
    }

    /// Let the CLI refresh an expired access token for `source` by running a
    /// minimal headless prompt against that config dir — the CLI owns
    /// refresh-token rotation, so we never call the OAuth endpoint ourselves.
    pub async fn refresh_via_cli(
        &self,
        env: &Environment,
        source: &CredSource,
    ) -> Result<(), AccountsError> {
        self.require_online(env).await?;
        let config_dir = match source {
            CredSource::Live => None,
            CredSource::Slot(id) => Some(self.slot_dir(env, id).await?),
        };
        run_headless_refresh(env, config_dir.as_deref()).await
    }
}

/// Resolve the Windows `claude` launcher (`claude.cmd` npm shim needs `cmd /C`).
fn local_claude_command() -> Result<tokio::process::Command, AccountsError> {
    let full = which::which("claude")
        .or_else(|_| which::which("claude.cmd"))
        .or_else(|_| which::which("claude.exe"))
        .map_err(|e| AccountsError::Spawn(format!("claude not on PATH: {e}")))?;
    let is_batch = full
        .extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "cmd" | "bat"));
    Ok(if is_batch {
        let mut c = tokio::process::Command::new("cmd.exe");
        c.arg("/C").arg(full);
        c
    } else {
        tokio::process::Command::new(full)
    })
}

/// POSIX single-quote for the `bash -lc` scripts below.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

async fn spawn_login_console(env: &Environment, config_dir: &str) -> Result<(), AccountsError> {
    // A visible console: the OAuth flow may print a URL to open by hand or ask
    // for a pasted code (always the case from inside WSL without a browser).
    #[cfg(windows)]
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let mut cmd = match env {
        Environment::Local => {
            let mut c = local_claude_command()?;
            c.args(["auth", "login", "--claudeai"])
                .env("CLAUDE_CONFIG_DIR", config_dir);
            c
        }
        Environment::Wsl { distro } => {
            let script = format!(
                "export CLAUDE_CONFIG_DIR={}; claude auth login --claudeai",
                sh_quote(config_dir)
            );
            let mut c = tokio::process::Command::new("wsl.exe");
            c.args(["-d", distro, "--", "bash", "-lic", &script]);
            c
        }
    };
    cmd.env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT");
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NEW_CONSOLE);
    let mut child = cmd
        .spawn()
        .map_err(|e| AccountsError::Spawn(format!("spawn claude auth login: {e}")))?;
    child
        .wait()
        .await
        .map_err(|e| AccountsError::Spawn(format!("wait claude auth login: {e}")))?;
    Ok(())
}

async fn run_headless_refresh(
    env: &Environment,
    config_dir: Option<&str>,
) -> Result<(), AccountsError> {
    use oxyris_procutil::HideConsole;
    // `-p .` on the cheapest model is the lightest request that makes the CLI
    // load + refresh its token; `--no-session-persistence` keeps it from
    // leaving a transcript behind.
    const ARGS: &[&str] = &["-p", ".", "--model", "haiku", "--no-session-persistence"];
    let mut cmd = match env {
        Environment::Local => {
            let mut c = local_claude_command()?;
            c.args(ARGS).current_dir(std::env::temp_dir());
            if let Some(dir) = config_dir {
                c.env("CLAUDE_CONFIG_DIR", dir);
            }
            c
        }
        Environment::Wsl { distro } => {
            let export = config_dir
                .map(|d| format!("export CLAUDE_CONFIG_DIR={}; ", sh_quote(d)))
                .unwrap_or_default();
            let script = format!("{export}cd ~ && claude {}", ARGS.join(" "));
            // `-i` too: nvm/pyenv PATH setup usually lives in `~/.bashrc`,
            // which a plain login shell skips (see `provider_discovery`).
            let mut c = tokio::process::Command::new("wsl.exe");
            c.args(["-d", distro, "--", "bash", "-lic", &script]);
            c
        }
    };
    cmd.env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .hide_console();
    let status = tokio::time::timeout(REFRESH_TIMEOUT, cmd.status())
        .await
        .map_err(|_| AccountsError::Spawn("claude token refresh timed out".into()))?
        .map_err(|e| AccountsError::Spawn(format!("spawn claude refresh: {e}")))?;
    if !status.success() {
        return Err(AccountsError::Spawn(format!(
            "claude token refresh exited with {}",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_keys_are_stable() {
        assert_eq!(env_key(&Environment::Local), "local");
        assert_eq!(
            env_key(&Environment::Wsl {
                distro: "Ubuntu".into()
            }),
            "wsl:Ubuntu"
        );
    }

    #[test]
    fn sh_quote_neutralizes_single_quotes() {
        assert_eq!(sh_quote("/home/a b"), "'/home/a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }

    fn login(dir: &Path, email: &str, token: &str) {
        std::fs::write(
            dir.join(CREDENTIALS_FILE),
            format!(
                r#"{{"claudeAiOauth":{{"accessToken":"{token}","refreshToken":"r-{token}","subscriptionType":"pro"}},"mcpOAuth":{{"gh":{{"t":"keep"}}}}}}"#
            ),
        )
        .unwrap();
        let config = std::fs::read_to_string(dir.join(CONFIG_FILE))
            .ok()
            .unwrap_or_else(|| r#"{"numStartups":7}"#.to_owned());
        let slot =
            format!(r#"{{"oauthAccount":{{"emailAddress":"{email}","accountUuid":"{email}"}}}}"#);
        std::fs::write(
            dir.join(CONFIG_FILE),
            docs::swap_oauth_account(Some(&config), &slot).unwrap(),
        )
        .unwrap();
    }

    fn read_json(path: PathBuf) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn save_switch_and_remove_round_trip() {
        let data = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        let mut accounts = ClaudeAccounts::new(
            data.path().to_path_buf(),
            Arc::new(AgentPool::new(PathBuf::from("unused"))),
        );
        accounts.local_live_dir = Some(live.path().to_path_buf());
        let env = Environment::Local;

        // Register A, then log in as B (what `claude /login` would do) and
        // register it too.
        login(live.path(), "a@x.com", "a1");
        let a = accounts.save_live(&env).await.unwrap();
        login(live.path(), "b@x.com", "b1");
        let b = accounts.save_live(&env).await.unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(b.plan, Plan::Pro);
        // Saving the same login again updates instead of duplicating.
        accounts.save_live(&env).await.unwrap();
        assert_eq!(accounts.records(&env).len(), 2);

        // B's token rotates while it's live, then we switch to A.
        login(live.path(), "b@x.com", "b2");
        accounts.activate(&env, &a.id).await.unwrap();

        let creds = read_json(live.path().join(CREDENTIALS_FILE));
        assert_eq!(creds["claudeAiOauth"]["accessToken"], "a1");
        assert_eq!(creds["mcpOAuth"]["gh"]["t"], "keep", "MCP tokens survive");
        let config = read_json(live.path().join(CONFIG_FILE));
        assert_eq!(config["oauthAccount"]["emailAddress"], "a@x.com");
        assert_eq!(config["numStartups"], 7, "history survives");
        let live_now = accounts.live(&env).await.unwrap().unwrap();
        assert_eq!(live_now.identity.email, "a@x.com");

        // The outgoing login was snapshotted with its rotated token.
        let b_slot = PathBuf::from(accounts.slot_dir(&env, &b.id).await.unwrap());
        let b_creds = read_json(b_slot.join(CREDENTIALS_FILE));
        assert_eq!(b_creds["claudeAiOauth"]["accessToken"], "b2");
        assert!(
            b_creds.get("mcpOAuth").is_none(),
            "slots hold no MCP tokens"
        );

        // Activating the already-live account is a no-op.
        accounts.activate(&env, &a.id).await.unwrap();

        // Switching back to B picks up the rotated token.
        accounts.activate(&env, &b.id).await.unwrap();
        let creds = read_json(live.path().join(CREDENTIALS_FILE));
        assert_eq!(creds["claudeAiOauth"]["accessToken"], "b2");

        // No temp files left behind anywhere.
        for dir in [live.path().to_path_buf(), b_slot.clone()] {
            let leftovers = std::fs::read_dir(&dir)
                .unwrap()
                .filter(|e| {
                    e.as_ref()
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".tmp")
                })
                .count();
            assert_eq!(leftovers, 0, "{}", dir.display());
        }

        // Removing the live account only unregisters it.
        accounts.remove(&env, &b.id).await.unwrap();
        assert!(!b_slot.exists());
        assert_eq!(accounts.records(&env).len(), 1);
        assert_eq!(
            accounts.live(&env).await.unwrap().unwrap().identity.email,
            "b@x.com"
        );
        assert!(matches!(
            accounts.remove(&env, &b.id).await,
            Err(AccountsError::NotFound)
        ));
    }

    #[tokio::test]
    async fn switching_away_from_unregistered_login_registers_it() {
        let data = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        let mut accounts = ClaudeAccounts::new(
            data.path().to_path_buf(),
            Arc::new(AgentPool::new(PathBuf::from("unused"))),
        );
        accounts.local_live_dir = Some(live.path().to_path_buf());
        let env = Environment::Local;

        login(live.path(), "a@x.com", "a1");
        let a = accounts.save_live(&env).await.unwrap();
        // User logs into C outside oxyris and never registers it.
        login(live.path(), "c@x.com", "c1");
        accounts.activate(&env, &a.id).await.unwrap();

        let records = accounts.records(&env);
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r.identity.email == "c@x.com"));
    }

    #[tokio::test]
    async fn no_live_login_reads_as_none() {
        let data = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        let mut accounts = ClaudeAccounts::new(
            data.path().to_path_buf(),
            Arc::new(AgentPool::new(PathBuf::from("unused"))),
        );
        accounts.local_live_dir = Some(live.path().to_path_buf());
        assert!(accounts.live(&Environment::Local).await.unwrap().is_none());
        assert!(matches!(
            accounts.save_live(&Environment::Local).await,
            Err(AccountsError::NoLiveLogin)
        ));
    }
}
