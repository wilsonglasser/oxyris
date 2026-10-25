//! Claude Code usage poller — the data behind the accounts panel and the
//! taskbar widget.
//!
//! Backend-owned: one loop polls `GET /api/oauth/usage` for every tracked
//! environment's accounts and emits [`BOARD_EVENT`] with the whole board, so
//! the main window and the widget window render the same snapshot without
//! each running their own timers.
//!
//! - Active accounts read the **live** login and poll every
//!   [`ACTIVE_INTERVAL_MS`]; reserves read their slot and poll every
//!   [`RESERVE_INTERVAL_MS`].
//! - `429` honours `Retry-After` (falling back to [`DEFAULT_BACKOFF_MS`]) and
//!   keeps showing the last good numbers.
//! - An expired access token is refreshed by the CLI itself
//!   (`ClaudeAccounts::refresh_via_cli`), at most once per
//!   [`CLI_REFRESH_COOLDOWN_MS`] per account — it costs a tiny prompt.
//!
//! Tracked environments: Windows always, plus every distro that has a
//! registered account or that the panel asked about this run. A distro is
//! never booted: while it isn't running its section is marked `offline` and
//! keeps the last known numbers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use oxyris_claude::accounts::{
    self as docs, AccountIdentity, OAuthCredentials, Plan, UsageSnapshot,
};
use oxyris_core::Environment;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::infra::claude_accounts::{AccountsError, ClaudeAccounts, CredSource, env_key};

pub const BOARD_EVENT: &str = "claude-usage://board";

const TICK: Duration = Duration::from_secs(30);
const ACTIVE_INTERVAL_MS: i64 = 90_000;
const RESERVE_INTERVAL_MS: i64 = 15 * 60_000;
const DEFAULT_BACKOFF_MS: i64 = 5 * 60_000;
const CLI_REFRESH_COOLDOWN_MS: i64 = 10 * 60_000;
/// Refresh a token this long before it actually expires.
const EXPIRY_SKEW_MS: i64 = 60_000;
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum UsageError {
    /// Login rejected even after a refresh — re-login needed.
    Auth(String),
    RateLimited(String),
    /// Token lacks `user:profile` (e.g. a `setup-token`), so usage is unreadable.
    NoScope,
    Network(String),
    Http(String),
    Credentials(String),
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageEntry {
    pub usage: Option<UsageSnapshot>,
    pub error: Option<UsageError>,
    pub fetched_at_ms: Option<i64>,
    #[serde(skip)]
    last_attempt_ms: i64,
    #[serde(skip)]
    backoff_until_ms: i64,
    #[serde(skip)]
    last_cli_refresh_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountCard {
    /// Registry id; `None` for a live login that isn't registered yet.
    pub id: Option<String>,
    pub identity: AccountIdentity,
    pub plan: Plan,
    pub active: bool,
    pub usage: Option<UsageEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvBoard {
    pub environment: Environment,
    pub key: String,
    /// Why the live login couldn't be read (agent down, malformed file...).
    pub live_error: Option<String>,
    /// Distro not running: nothing was read; `accounts` are the last known
    /// cards (stale usage) so the panel and widget keep showing something.
    pub offline: bool,
    pub accounts: Vec<AccountCard>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageBoard {
    pub environments: Vec<EnvBoard>,
    pub updated_at_ms: i64,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, UsageEntry>,
    tracked: Vec<Environment>,
    board: Option<UsageBoard>,
    /// When each environment's live login + registry were last re-read. For
    /// a distro every re-read is agent round-trips, so a tick inside
    /// [`ACTIVE_INTERVAL_MS`] reuses the previous section instead.
    env_checked_ms: HashMap<String, i64>,
}

pub struct ClaudeUsage {
    accounts: Arc<ClaudeAccounts>,
    http: reqwest::Client,
    state: Mutex<State>,
    /// One refresh cycle at a time; a forced refresh waits for a running tick.
    cycle: Mutex<()>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn entry_key(env: &Environment, id: &AccountIdentity) -> String {
    format!(
        "{}|{}|{}",
        env_key(env),
        id.email.to_ascii_lowercase(),
        id.organization_uuid
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
    )
}

/// Seconds from a `Retry-After` header (delta-seconds form only; the
/// HTTP-date form falls back to the default backoff).
fn retry_after_ms(headers: &reqwest::header::HeaderMap) -> i64 {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<i64>().ok())
        .map(|s| s.clamp(30, 3600) * 1000)
        .unwrap_or(DEFAULT_BACKOFF_MS)
}

enum Fetch {
    Ok(UsageSnapshot),
    Unauthorized,
    Failed(UsageError, Option<i64>),
}

impl ClaudeUsage {
    pub fn new(accounts: Arc<ClaudeAccounts>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(concat!("oxyris/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        let mut tracked = vec![Environment::Local];
        for env in accounts.registered_environments() {
            if !tracked.contains(&env) {
                tracked.push(env);
            }
        }
        Self {
            accounts,
            http,
            state: Mutex::new(State {
                tracked,
                ..State::default()
            }),
            cycle: Mutex::new(()),
        }
    }

    pub fn accounts(&self) -> &ClaudeAccounts {
        &self.accounts
    }

    /// Start polling `env` (idempotent).
    pub async fn track(&self, env: &Environment) {
        let mut st = self.state.lock().await;
        if !st.tracked.contains(env) {
            st.tracked.push(env.clone());
        }
    }

    /// Last built board, or a fresh one when nothing has run yet.
    pub async fn board(&self, app: &AppHandle) -> UsageBoard {
        if let Some(b) = self.state.lock().await.board.clone() {
            return b;
        }
        self.refresh(app, false).await
    }

    /// Run the poller forever. Spawned once at boot.
    pub async fn run(self: Arc<Self>, app: AppHandle) {
        tokio::time::sleep(Duration::from_secs(5)).await;
        loop {
            self.refresh(&app, false).await;
            tokio::time::sleep(TICK).await;
        }
    }

    /// One cycle: rebuild the board, fetching whatever is due (or everything
    /// not backed off when `force`). Emits [`BOARD_EVENT`].
    pub async fn refresh(&self, app: &AppHandle, force: bool) -> UsageBoard {
        let _cycle = self.cycle.lock().await;
        let tracked = self.state.lock().await.tracked.clone();
        let mut board = UsageBoard {
            environments: Vec::with_capacity(tracked.len()),
            updated_at_ms: 0,
        };
        let mut refreshed_any = false;
        for env in &tracked {
            let key = env_key(env);
            let reuse = if force {
                None
            } else {
                let st = self.state.lock().await;
                let fresh = st
                    .env_checked_ms
                    .get(&key)
                    .is_some_and(|t| now_ms() - t < ACTIVE_INTERVAL_MS);
                fresh
                    .then(|| st.board.as_ref())
                    .flatten()
                    .and_then(|b| b.environments.iter().find(|e| e.key == key).cloned())
            };
            let section = match reuse {
                Some(section) => section,
                None => {
                    let section = self.refresh_env(env, force).await;
                    self.state.lock().await.env_checked_ms.insert(key, now_ms());
                    refreshed_any = true;
                    section
                }
            };
            board.environments.push(section);
        }
        let mut st = self.state.lock().await;
        if !refreshed_any && let Some(previous) = &st.board {
            // Nothing re-read this tick: skip the emit (and the widget repaint).
            return previous.clone();
        }
        board.updated_at_ms = now_ms();
        st.board = Some(board.clone());
        drop(st);
        let _ = app.emit(BOARD_EVENT, &board);
        board
    }

    async fn refresh_env(&self, env: &Environment, force: bool) -> EnvBoard {
        if !self.accounts.is_online(env).await {
            return self.offline_section(env).await;
        }
        let (live, live_error) = match self.accounts.live(env).await {
            Ok(l) => (l, None),
            Err(e) => (None, Some(e.to_string())),
        };
        let mut cards: Vec<(AccountCard, CredSource)> = self
            .accounts
            .records(env)
            .into_iter()
            .map(|r| {
                let active = live
                    .as_ref()
                    .is_some_and(|l| l.identity.same_account(&r.identity));
                let source = if active {
                    CredSource::Live
                } else {
                    CredSource::Slot(r.id.clone())
                };
                (
                    AccountCard {
                        id: Some(r.id),
                        identity: r.identity,
                        // The live file knows the current plan better than
                        // the snapshot taken at registration.
                        plan: match &live {
                            Some(l) if active => l.plan,
                            _ => r.plan,
                        },
                        active,
                        usage: None,
                    },
                    source,
                )
            })
            .collect();
        if let Some(l) = &live
            && !cards.iter().any(|(c, _)| c.active)
        {
            cards.insert(
                0,
                (
                    AccountCard {
                        id: None,
                        identity: l.identity.clone(),
                        plan: l.plan,
                        active: true,
                        usage: None,
                    },
                    CredSource::Live,
                ),
            );
        }
        // Active first, then reserves in registration order.
        cards.sort_by_key(|(c, _)| !c.active);

        for (card, source) in &mut cards {
            let key = entry_key(env, &card.identity);
            let interval = if card.active {
                ACTIVE_INTERVAL_MS
            } else {
                RESERVE_INTERVAL_MS
            };
            let mut entry = self
                .state
                .lock()
                .await
                .entries
                .get(&key)
                .cloned()
                .unwrap_or_default();
            let now = now_ms();
            let due =
                now >= entry.backoff_until_ms && (force || now - entry.last_attempt_ms >= interval);
            if due {
                self.fetch_into(env, source, &mut entry).await;
                self.state.lock().await.entries.insert(key, entry.clone());
            }
            card.usage = Some(entry);
        }

        EnvBoard {
            environment: env.clone(),
            key: env_key(env),
            live_error,
            offline: false,
            accounts: cards.into_iter().map(|(c, _)| c).collect(),
        }
    }

    /// Section for a distro that isn't running: the previous cards when we
    /// have them, else the registry with no usage. Never touches the distro.
    async fn offline_section(&self, env: &Environment) -> EnvBoard {
        let key = env_key(env);
        let previous = self
            .state
            .lock()
            .await
            .board
            .as_ref()
            .and_then(|b| b.environments.iter().find(|e| e.key == key).cloned());
        let accounts = match previous {
            Some(section) => section.accounts,
            None => self
                .accounts
                .records(env)
                .into_iter()
                .map(|r| AccountCard {
                    id: Some(r.id),
                    identity: r.identity,
                    plan: r.plan,
                    active: false,
                    usage: None,
                })
                .collect(),
        };
        EnvBoard {
            environment: env.clone(),
            key,
            live_error: None,
            offline: true,
            accounts,
        }
    }

    async fn fetch_into(&self, env: &Environment, source: &CredSource, entry: &mut UsageEntry) {
        entry.last_attempt_ms = now_ms();
        let mut creds = match self.accounts.credentials(env, source).await {
            Ok(c) => c,
            Err(e) => {
                entry.error = Some(UsageError::Credentials(e.to_string()));
                return;
            }
        };
        if !creds.has_usage_scopes() {
            entry.error = Some(UsageError::NoScope);
            entry.usage = None;
            return;
        }
        let mut refreshed = false;
        if creds.is_expiring(now_ms(), EXPIRY_SKEW_MS) {
            match self.cli_refresh(env, source, entry).await {
                Some(Ok(c)) => {
                    creds = c;
                    refreshed = true;
                }
                Some(Err(e)) => tracing::warn!(env = %env_key(env), "claude token refresh: {e}"),
                None => {}
            }
        }
        let mut result = self.fetch(&creds).await;
        if matches!(result, Fetch::Unauthorized)
            && !refreshed
            && let Some(Ok(c)) = self.cli_refresh(env, source, entry).await
        {
            result = self.fetch(&c).await;
        }
        match result {
            Fetch::Ok(usage) => {
                entry.usage = Some(usage);
                entry.error = None;
                entry.fetched_at_ms = Some(now_ms());
            }
            Fetch::Unauthorized => {
                entry.error = Some(UsageError::Auth(
                    "login rejected; sign in again for this account".into(),
                ));
            }
            Fetch::Failed(err, backoff) => {
                if let Some(ms) = backoff {
                    entry.backoff_until_ms = now_ms() + ms;
                }
                entry.error = Some(err);
            }
        }
    }

    /// CLI refresh honouring the per-account cooldown. `None` = skipped.
    async fn cli_refresh(
        &self,
        env: &Environment,
        source: &CredSource,
        entry: &mut UsageEntry,
    ) -> Option<Result<OAuthCredentials, AccountsError>> {
        let now = now_ms();
        if now - entry.last_cli_refresh_ms < CLI_REFRESH_COOLDOWN_MS {
            return None;
        }
        entry.last_cli_refresh_ms = now;
        Some(match self.accounts.refresh_via_cli(env, source).await {
            Ok(()) => self.accounts.credentials(env, source).await,
            Err(e) => Err(e),
        })
    }

    async fn fetch(&self, creds: &OAuthCredentials) -> Fetch {
        let resp = match self
            .http
            .get(docs::USAGE_URL)
            .bearer_auth(creds.access_token())
            .header("anthropic-beta", docs::USAGE_BETA_HEADER)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return Fetch::Failed(UsageError::Network(e.to_string()), None),
        };
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Fetch::Unauthorized;
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let backoff = retry_after_ms(resp.headers());
            return Fetch::Failed(
                UsageError::RateLimited(format!("retry in {}s", backoff / 1000)),
                Some(backoff),
            );
        }
        if !status.is_success() {
            // Server errors back off a little so a flapping endpoint isn't
            // hammered every tick.
            let backoff = status.is_server_error().then_some(60_000);
            return Fetch::Failed(UsageError::Http(status.to_string()), backoff);
        }
        match resp.text().await {
            Ok(body) => match docs::parse_usage(&body) {
                Ok(u) => Fetch::Ok(u),
                Err(e) => Fetch::Failed(UsageError::Http(format!("unexpected body: {e}")), None),
            },
            Err(e) => Fetch::Failed(UsageError::Network(e.to_string()), None),
        }
    }

    /// Rebuild after a switch/save/remove without making the caller wait on
    /// a poller cycle that may be stuck in a slow CLI refresh: past a short
    /// timeout, hand back the cached board and let a background cycle emit
    /// the fresh one.
    pub async fn refresh_after_change(self: &Arc<Self>, app: &AppHandle) -> UsageBoard {
        match tokio::time::timeout(Duration::from_secs(3), self.refresh(app, false)).await {
            Ok(board) => board,
            Err(_) => {
                let me = self.clone();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    me.refresh(&app, false).await;
                });
                self.state.lock().await.board.clone().unwrap_or_default()
            }
        }
    }

    /// Drop cached state for an environment so the next cycle re-fetches
    /// immediately (after a switch, save or removal).
    pub async fn invalidate(&self, env: &Environment) {
        let prefix = format!("{}|", env_key(env));
        let mut st = self.state.lock().await;
        st.env_checked_ms.remove(&env_key(env));
        for (k, e) in st.entries.iter_mut() {
            if k.starts_with(&prefix) {
                e.last_attempt_ms = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_parses_seconds_and_clamps() {
        let mut h = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after_ms(&h), DEFAULT_BACKOFF_MS);
        h.insert(reqwest::header::RETRY_AFTER, "120".parse().unwrap());
        assert_eq!(retry_after_ms(&h), 120_000);
        h.insert(reqwest::header::RETRY_AFTER, "1".parse().unwrap());
        assert_eq!(retry_after_ms(&h), 30_000);
        h.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after_ms(&h), DEFAULT_BACKOFF_MS);
    }

    #[test]
    fn entry_key_ignores_email_case_and_separates_envs() {
        let id = AccountIdentity {
            email: "Me@X.com".into(),
            display_name: None,
            account_uuid: "a".into(),
            organization_uuid: Some("ORG".into()),
            organization_name: None,
        };
        let wsl = Environment::Wsl {
            distro: "Ubuntu".into(),
        };
        assert_eq!(entry_key(&Environment::Local, &id), "local|me@x.com|org");
        assert_ne!(entry_key(&Environment::Local, &id), entry_key(&wsl, &id));
    }

    #[test]
    fn board_serializes_without_tokens() {
        let card = AccountCard {
            id: Some("1".into()),
            identity: AccountIdentity {
                email: "me@x.com".into(),
                display_name: None,
                account_uuid: "a".into(),
                organization_uuid: None,
                organization_name: None,
            },
            plan: Plan::Pro,
            active: true,
            usage: Some(UsageEntry {
                error: Some(UsageError::NoScope),
                ..UsageEntry::default()
            }),
        };
        let json = serde_json::to_string(&card).unwrap();
        assert!(!json.contains("token"), "{json}");
        assert!(json.contains(r#""kind":"no_scope""#), "{json}");
    }
}
