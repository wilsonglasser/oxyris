//! Pure parsers for the Claude Code account files and the OAuth usage API.
//!
//! The CLI keeps an account's identity in two documents inside its config
//! directory (`~/.claude` by default, or `$CLAUDE_CONFIG_DIR`):
//!
//! - `.credentials.json` → `claudeAiOauth` holds the OAuth token pair plus the
//!   plan (`subscriptionType`, `rateLimitTier`). Sibling keys such as
//!   `mcpOAuth` belong to MCP servers and are **not** account state.
//! - `.claude.json` → `oauthAccount` holds who is logged in (email, account
//!   and organization UUIDs). Everything else in that file is per-machine
//!   history/projects/MCP config.
//!
//! Switching accounts therefore swaps exactly those two subtrees and leaves
//! the rest untouched — see [`swap_oauth_credentials`] and
//! [`swap_oauth_account`]. No I/O happens here; the desktop backend routes the
//! reads/writes per environment (native fs vs the WSL agent).
//!
//! Token material never leaves this module through `Serialize`/`Debug`:
//! [`OAuthCredentials`] redacts itself, and only [`OAuthCredentials::access_token`]
//! hands the bearer out, for the usage request.
//!
//! Usage is Claude-specific today. When a second provider grows quotas this
//! is the seam to lift into `oxyris-provider` as a capability.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

/// Usage endpoint read with the account's OAuth access token.
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// Beta header the usage endpoint requires for OAuth bearer tokens.
pub const USAGE_BETA_HEADER: &str = "oauth-2025-04-20";

const CREDENTIALS_KEY: &str = "claudeAiOauth";
const ACCOUNT_KEY: &str = "oauthAccount";
/// Values at or above this are milliseconds since the epoch; below, seconds.
const MILLIS_THRESHOLD: i64 = 10_000_000_000;

#[derive(Debug, Error)]
pub enum AccountDocError {
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("document is not a JSON object")]
    NotAnObject,
    #[error("missing `{0}`")]
    Missing(&'static str),
}

/// The `claudeAiOauth` block of `.credentials.json`.
#[derive(Clone)]
pub struct OAuthCredentials {
    access_token: String,
    has_refresh_token: bool,
    /// Normalized to milliseconds.
    pub expires_at_ms: Option<i64>,
    pub scopes: Vec<String>,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
}

impl std::fmt::Debug for OAuthCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthCredentials")
            .field("access_token", &"<redacted>")
            .field("has_refresh_token", &self.has_refresh_token)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("scopes", &self.scopes)
            .field("subscription_type", &self.subscription_type)
            .field("rate_limit_tier", &self.rate_limit_tier)
            .finish()
    }
}

impl OAuthCredentials {
    /// Bearer for the usage request. Never log or serialize it.
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn has_refresh_token(&self) -> bool {
        self.has_refresh_token
    }

    /// True when the access token is expired or expires within `skew_ms`.
    /// Unknown expiry counts as fresh — the usage call's 401 is the backstop.
    pub fn is_expiring(&self, now_ms: i64, skew_ms: i64) -> bool {
        self.expires_at_ms
            .is_some_and(|exp| exp <= now_ms.saturating_add(skew_ms))
    }

    /// The usage endpoint needs both `user:profile` and `user:inference`.
    /// Tokens minted by `claude setup-token` carry only inference, so usage is
    /// unavailable for them rather than an error. An empty list is treated as
    /// "unknown, try anyway" (older CLIs didn't persist scopes).
    pub fn has_usage_scopes(&self) -> bool {
        self.scopes.is_empty()
            || (self.scopes.iter().any(|s| s == "user:profile")
                && self.scopes.iter().any(|s| s == "user:inference"))
    }

    pub fn plan(&self) -> Plan {
        Plan::from_parts(
            self.subscription_type.as_deref(),
            self.rate_limit_tier.as_deref(),
        )
    }
}

/// The `oauthAccount` block of `.claude.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountIdentity {
    pub email: String,
    pub display_name: Option<String>,
    pub account_uuid: String,
    pub organization_uuid: Option<String>,
    pub organization_name: Option<String>,
}

impl AccountIdentity {
    /// Same login: one email can belong to several organizations, each with
    /// its own quota, so the org is part of the key.
    pub fn same_account(&self, other: &AccountIdentity) -> bool {
        self.email.eq_ignore_ascii_case(&other.email)
            && self
                .organization_uuid
                .as_deref()
                .map(str::to_ascii_lowercase)
                == other
                    .organization_uuid
                    .as_deref()
                    .map(str::to_ascii_lowercase)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Plan {
    Pro,
    Max5x,
    Max20x,
    Max,
    Team,
    Enterprise,
    Free,
    Unknown,
}

impl Plan {
    pub fn from_parts(subscription_type: Option<&str>, rate_limit_tier: Option<&str>) -> Self {
        let sub = subscription_type.unwrap_or("").trim().to_ascii_lowercase();
        let tier = rate_limit_tier.unwrap_or("").trim().to_ascii_lowercase();
        match sub.as_str() {
            "max" if tier.ends_with("max_20x") => Plan::Max20x,
            "max" if tier.ends_with("max_5x") => Plan::Max5x,
            "max" => Plan::Max,
            "pro" => Plan::Pro,
            "team" => Plan::Team,
            "enterprise" => Plan::Enterprise,
            "free" => Plan::Free,
            _ => Plan::Unknown,
        }
    }
}

/// `expiresAt` has been written both as seconds and milliseconds by
/// different CLI versions; normalize to milliseconds.
pub fn normalize_epoch_ms(value: i64) -> i64 {
    if value > 0 && value < MILLIS_THRESHOLD {
        value.saturating_mul(1000)
    } else {
        value
    }
}

fn parse_object(json: &str) -> Result<Map<String, Value>, AccountDocError> {
    match serde_json::from_str::<Value>(json)? {
        Value::Object(map) => Ok(map),
        _ => Err(AccountDocError::NotAnObject),
    }
}

fn str_field(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn i64_field(obj: &Map<String, Value>, key: &str) -> Option<i64> {
    match obj.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Parse `.credentials.json`.
pub fn parse_credentials(json: &str) -> Result<OAuthCredentials, AccountDocError> {
    let root = parse_object(json)?;
    let oauth = root
        .get(CREDENTIALS_KEY)
        .and_then(Value::as_object)
        .ok_or(AccountDocError::Missing(CREDENTIALS_KEY))?;
    let access_token =
        str_field(oauth, "accessToken").ok_or(AccountDocError::Missing("accessToken"))?;
    let scopes = oauth
        .get("scopes")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok(OAuthCredentials {
        access_token,
        has_refresh_token: str_field(oauth, "refreshToken").is_some(),
        expires_at_ms: i64_field(oauth, "expiresAt").map(normalize_epoch_ms),
        scopes,
        subscription_type: str_field(oauth, "subscriptionType"),
        rate_limit_tier: str_field(oauth, "rateLimitTier"),
    })
}

/// Parse the `oauthAccount` block out of `.claude.json`.
pub fn parse_identity(json: &str) -> Result<AccountIdentity, AccountDocError> {
    let root = parse_object(json)?;
    let account = root
        .get(ACCOUNT_KEY)
        .and_then(Value::as_object)
        .ok_or(AccountDocError::Missing(ACCOUNT_KEY))?;
    Ok(AccountIdentity {
        email: str_field(account, "emailAddress")
            .ok_or(AccountDocError::Missing("emailAddress"))?,
        display_name: str_field(account, "displayName"),
        account_uuid: str_field(account, "accountUuid")
            .ok_or(AccountDocError::Missing("accountUuid"))?,
        organization_uuid: str_field(account, "organizationUuid"),
        organization_name: str_field(account, "organizationName"),
    })
}

/// Copy `key` from `source` into `target` (or an empty object when `target`
/// is absent), leaving every other key in `target` as it was.
fn swap_subtree(
    target: Option<&str>,
    source: &str,
    key: &'static str,
) -> Result<String, AccountDocError> {
    let source = parse_object(source)?;
    let subtree = source
        .get(key)
        .filter(|v| v.is_object())
        .cloned()
        .ok_or(AccountDocError::Missing(key))?;
    let mut target = match target.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => parse_object(t)?,
        None => Map::new(),
    };
    target.insert(key.to_owned(), subtree);
    Ok(serde_json::to_string_pretty(&Value::Object(target))?)
}

/// New `.credentials.json` for the live config dir: `claudeAiOauth` from the
/// slot, everything else (e.g. `mcpOAuth`) kept from the live file.
pub fn swap_oauth_credentials(live: Option<&str>, slot: &str) -> Result<String, AccountDocError> {
    swap_subtree(live, slot, CREDENTIALS_KEY)
}

/// New `.claude.json` for the live config dir: `oauthAccount` from the slot,
/// history/projects/MCP config kept from the live file.
pub fn swap_oauth_account(live: Option<&str>, slot: &str) -> Result<String, AccountDocError> {
    swap_subtree(live, slot, ACCOUNT_KEY)
}

/// One rate-limit window from the usage endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    /// Percent used, 0–100 (may exceed 100 when over the limit).
    pub utilization: f64,
    /// RFC 3339 timestamp of the next reset, when the window is running.
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
    pub seven_day_opus: Option<UsageWindow>,
    pub seven_day_sonnet: Option<UsageWindow>,
}

fn window(root: &Map<String, Value>, key: &str) -> Option<UsageWindow> {
    let obj = root.get(key)?.as_object()?;
    let utilization = match obj.get("utilization")? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse().ok()?,
        _ => return None,
    };
    Some(UsageWindow {
        utilization,
        resets_at: str_field(obj, "resets_at"),
    })
}

/// Parse the `GET /api/oauth/usage` body. Unknown keys are ignored and
/// missing/null windows come back as `None`, so a schema addition upstream
/// never breaks the panel.
pub fn parse_usage(json: &str) -> Result<UsageSnapshot, AccountDocError> {
    let root = parse_object(json)?;
    Ok(UsageSnapshot {
        five_hour: window(&root, "five_hour"),
        seven_day: window(&root, "seven_day"),
        seven_day_opus: window(&root, "seven_day_opus"),
        seven_day_sonnet: window(&root, "seven_day_sonnet"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREDS: &str = r#"{
        "claudeAiOauth": {
            "accessToken": "sk-ant-oat01-secret",
            "refreshToken": "sk-ant-ort01-secret",
            "expiresAt": 1761350400000,
            "scopes": ["user:inference", "user:profile", "user:sessions:claude_code"],
            "subscriptionType": "max",
            "rateLimitTier": "default_claude_max_20x"
        },
        "mcpOAuth": { "github": { "token": "keep-me" } }
    }"#;

    const CONFIG: &str = r#"{
        "numStartups": 42,
        "projects": { "/home/me/app": { "history": [] } },
        "oauthAccount": {
            "emailAddress": "me@example.com",
            "displayName": "Me",
            "accountUuid": "acc-1",
            "organizationUuid": "ORG-1",
            "organizationName": "Mine"
        }
    }"#;

    #[test]
    fn parses_credentials_and_plan() {
        let c = parse_credentials(CREDS).unwrap();
        assert_eq!(c.access_token(), "sk-ant-oat01-secret");
        assert!(c.has_refresh_token());
        assert_eq!(c.expires_at_ms, Some(1_761_350_400_000));
        assert!(c.has_usage_scopes());
        assert_eq!(c.plan(), Plan::Max20x);
    }

    #[test]
    fn debug_redacts_tokens() {
        let c = parse_credentials(CREDS).unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("secret"), "{dbg}");
    }

    #[test]
    fn expires_at_seconds_are_normalized() {
        let json = r#"{"claudeAiOauth":{"accessToken":"a","expiresAt":1761350400}}"#;
        let c = parse_credentials(json).unwrap();
        assert_eq!(c.expires_at_ms, Some(1_761_350_400_000));
        assert!(c.is_expiring(1_761_350_400_000 - 1000, 5_000));
        assert!(!c.is_expiring(1_761_350_400_000 - 10_000, 5_000));
    }

    #[test]
    fn inference_only_token_has_no_usage_scopes() {
        let json = r#"{"claudeAiOauth":{"accessToken":"a","scopes":["user:inference"]}}"#;
        assert!(!parse_credentials(json).unwrap().has_usage_scopes());
    }

    #[test]
    fn missing_oauth_block_is_an_error() {
        assert!(matches!(
            parse_credentials(r#"{"mcpOAuth":{}}"#),
            Err(AccountDocError::Missing("claudeAiOauth"))
        ));
        assert!(matches!(
            parse_credentials("[]"),
            Err(AccountDocError::NotAnObject)
        ));
    }

    #[test]
    fn parses_identity_and_matches_case_insensitively() {
        let id = parse_identity(CONFIG).unwrap();
        assert_eq!(id.email, "me@example.com");
        assert_eq!(id.organization_uuid.as_deref(), Some("ORG-1"));
        let other = AccountIdentity {
            email: "ME@example.com".into(),
            display_name: None,
            account_uuid: "acc-1".into(),
            organization_uuid: Some("org-1".into()),
            organization_name: None,
        };
        assert!(id.same_account(&other));
        let other_org = AccountIdentity {
            organization_uuid: Some("org-2".into()),
            ..other
        };
        assert!(!id.same_account(&other_org));
    }

    #[test]
    fn swap_credentials_keeps_mcp_tokens() {
        let slot = r#"{"claudeAiOauth":{"accessToken":"other","refreshToken":"r2"}}"#;
        let out = swap_oauth_credentials(Some(CREDS), slot).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["claudeAiOauth"]["accessToken"], "other");
        assert_eq!(v["mcpOAuth"]["github"]["token"], "keep-me");
    }

    #[test]
    fn swap_credentials_without_live_file() {
        let slot = r#"{"claudeAiOauth":{"accessToken":"other"}}"#;
        let out = swap_oauth_credentials(None, slot).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["claudeAiOauth"]["accessToken"], "other");
    }

    #[test]
    fn swap_account_keeps_history() {
        let slot = r#"{"oauthAccount":{"emailAddress":"b@example.com","accountUuid":"acc-2"}}"#;
        let out = swap_oauth_account(Some(CONFIG), slot).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["oauthAccount"]["emailAddress"], "b@example.com");
        assert_eq!(v["numStartups"], 42);
        assert!(v["projects"]["/home/me/app"].is_object());
    }

    #[test]
    fn swap_rejects_slot_without_subtree() {
        assert!(swap_oauth_account(Some(CONFIG), "{}").is_err());
    }

    #[test]
    fn parses_usage_with_nulls_and_extras() {
        let json = r#"{
            "five_hour": {"utilization": 37.0, "resets_at": "2026-10-25T03:00:00+00:00"},
            "seven_day": {"utilization": 12, "resets_at": null},
            "seven_day_opus": null,
            "extra_usage": {"is_enabled": false}
        }"#;
        let u = parse_usage(json).unwrap();
        assert_eq!(u.five_hour.as_ref().unwrap().utilization, 37.0);
        assert_eq!(
            u.five_hour.unwrap().resets_at.as_deref(),
            Some("2026-10-25T03:00:00+00:00")
        );
        assert_eq!(u.seven_day.unwrap().resets_at, None);
        assert!(u.seven_day_opus.is_none());
        assert!(u.seven_day_sonnet.is_none());
    }

    #[test]
    fn plan_mapping() {
        assert_eq!(Plan::from_parts(Some("pro"), None), Plan::Pro);
        assert_eq!(
            Plan::from_parts(Some("max"), Some("default_claude_max_5x")),
            Plan::Max5x
        );
        assert_eq!(Plan::from_parts(Some("max"), None), Plan::Max);
        assert_eq!(Plan::from_parts(None, None), Plan::Unknown);
    }
}
