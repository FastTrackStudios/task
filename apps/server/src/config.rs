//! The server's environment, in one place: [`ServerConfig`].
//!
//! Every variable the server's boot and router (`lib.rs`, `main.rs`)
//! read is a field here, with the background loops' own switches
//! (`device_sync`, `wiki_repo`), under the name it has always had
//! (`TASK_*`, plus the few unprefixed fallbacks such as
//! `FORGEJO_TOKEN`), through `#[derive(architect::Config)]`. Nothing was
//! renamed, and the integration test `server_config` pins every name and
//! every default. Feature modules that read their own variables
//! (`telemetry_query`, `central_auth`, the CLIs, ...) still do; moving
//! them in is a field each.
//!
//! Every field is a string. A value is still parsed where it is used
//! (a bind address, a TTL, a `1` that turns enforcement on), so a
//! malformed value fails or falls back exactly where it did before,
//! and [`ServerConfig::from_env`] has no error path in practice.
//!
//! Defaults that depend on the org being booted (its sqlite paths, its
//! vault and wiki dirs) stay at the use site, because a config read from
//! the environment cannot know which org it is for. Those fields are
//! `Option`s, and `None` means "use the org's own path".
//!
//! One change in reading comes with the derive: a value is trimmed, and
//! a blank value counts as unset (`architect::config::var`). Before,
//! `TASK_SERVER_VAULT_ROOT=` pointed the vault at the empty path, and a
//! blank `TASK_ESV_API_KEY` hid a set `ESV_API_KEY`; now both fall
//! through to the default or the fallback.

/// What `TASK_SERVER_BIND` defaults to for the listener (`main.rs`).
pub const DEFAULT_LISTEN_BIND: &str = "0.0.0.0:9090";

/// What `TASK_SERVER_BIND` defaults to when a share link has no public
/// base to compose against ([`crate::share_public_base`]). Not the
/// listener's default: this predates it and links were never rebased.
pub const DEFAULT_SHARE_BIND: &str = "127.0.0.1:3456";

/// What `TASK_BUILD_REV` defaults to outside the flake's image build.
pub const DEFAULT_BUILD_REV: &str = "unknown";

/// Every environment variable the server reads, typed once.
///
/// Read it with [`ServerConfig::load`] where the value is needed: the
/// server has always read its environment late (per org boot, some of
/// it per request), and the tests rely on that, so nothing here caches.
#[derive(architect::Config, Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    // ── listener and process ────────────────────────────────────────
    /// `TASK_SERVER_BIND`: the listener address. Two defaults, one per
    /// use: [`Self::listen_bind`] and [`Self::share_bind`].
    #[architect(env = "TASK_SERVER_BIND")]
    pub bind: Option<String>,
    /// `TASK_SERVER_ORG`: host just this org (legacy single-org boot).
    #[architect(env = "TASK_SERVER_ORG")]
    pub org: Option<String>,
    /// `TASK_SERVER_WEB_DIR`: a dx web build served same-origin.
    #[architect(env = "TASK_SERVER_WEB_DIR")]
    pub web_dir: Option<String>,
    /// `TASK_BUILD_REV`: the git rev `/health` reports.
    #[architect(env = "TASK_BUILD_REV", default = DEFAULT_BUILD_REV)]
    pub build_rev: String,
    /// `TASK_AUTH_SECRET`: signs every org's session tokens.
    #[architect(env = "TASK_AUTH_SECRET")]
    pub auth_secret: Option<String>,
    /// `TASK_CORS_ALLOWED_ORIGINS`: comma-separated `Origin` allowlist.
    #[architect(env = "TASK_CORS_ALLOWED_ORIGINS")]
    pub cors_allowed_origins: Option<String>,
    /// `TASK_ENFORCE_PERMISSIONS`: the gate enforces only on exactly `1`.
    #[architect(env = "TASK_ENFORCE_PERMISSIONS")]
    pub enforce_permissions: Option<String>,
    /// `TASK_ENFORCE_MEDIA_TOKEN`: the media route enforces only on `1`.
    #[architect(env = "TASK_ENFORCE_MEDIA_TOKEN")]
    pub enforce_media_token: Option<String>,

    // ── public URLs ─────────────────────────────────────────────────
    /// `TASK_SERVER_PUBLIC_URL`: base for signed attachment URLs.
    #[architect(env = "TASK_SERVER_PUBLIC_URL")]
    pub public_url: Option<String>,
    /// `TASK_SHARE_PUBLIC_BASE`: base for share links; wins over
    /// `TASK_SERVER_PUBLIC_URL`.
    #[architect(env = "TASK_SHARE_PUBLIC_BASE")]
    pub share_public_base: Option<String>,

    // ── per-org path overrides (default: the org's own path) ────────
    /// `TASK_SERVER_VAULT_ROOT`.
    #[architect(env = "TASK_SERVER_VAULT_ROOT")]
    pub vault_root: Option<String>,
    /// `TASK_SERVER_WIKI_ROOT`: pins a single `knowledge` wiki.
    #[architect(env = "TASK_SERVER_WIKI_ROOT")]
    pub wiki_root: Option<String>,
    /// `TASK_SERVER_CRDT_ROOT`.
    #[architect(env = "TASK_SERVER_CRDT_ROOT")]
    pub crdt_root: Option<String>,
    /// `TASK_SERVER_MAIL_ROOT`.
    #[architect(env = "TASK_SERVER_MAIL_ROOT")]
    pub mail_root: Option<String>,
    /// `TASK_SERVER_AGENT_TASKS_URL`.
    #[architect(env = "TASK_SERVER_AGENT_TASKS_URL")]
    pub agent_tasks_url: Option<String>,
    /// `TASK_SERVER_AGENT_RUNNERS_URL`.
    #[architect(env = "TASK_SERVER_AGENT_RUNNERS_URL")]
    pub agent_runners_url: Option<String>,
    /// `TASK_SERVER_TIMER_URL`.
    #[architect(env = "TASK_SERVER_TIMER_URL")]
    pub timer_url: Option<String>,
    /// `TASK_SERVER_THREADS_URL`.
    #[architect(env = "TASK_SERVER_THREADS_URL")]
    pub threads_url: Option<String>,
    /// `TASK_SERVER_NOTIFY_URL`.
    #[architect(env = "TASK_SERVER_NOTIFY_URL")]
    pub notify_url: Option<String>,
    /// `TASK_SERVER_PREFS_URL`.
    #[architect(env = "TASK_SERVER_PREFS_URL")]
    pub prefs_url: Option<String>,
    /// `TASK_SERVER_FINANCE_URL`.
    #[architect(env = "TASK_SERVER_FINANCE_URL")]
    pub finance_url: Option<String>,

    // ── wiki ────────────────────────────────────────────────────────
    /// `TASK_WIKI_DOMAINS`: `slug=domain` overrides for wiki homes.
    #[architect(env = "TASK_WIKI_DOMAINS")]
    pub wiki_domains: Option<String>,
    /// `TASK_WIKI_CLAIM_TTL_SECS`: how long an Edit Request claim stands.
    #[architect(env = "TASK_WIKI_CLAIM_TTL_SECS")]
    pub wiki_claim_ttl_secs: Option<String>,
    /// `TASK_WIKI_REPO_SYNC_SECS`: the repo-sourced wiki sync interval.
    #[architect(env = "TASK_WIKI_REPO_SYNC_SECS")]
    pub wiki_repo_sync_secs: Option<String>,

    // ── device sync ─────────────────────────────────────────────────
    /// `TASK_DEVICE_SYNC_SECS`: the device sweep interval; `0` is off.
    #[architect(env = "TASK_DEVICE_SYNC_SECS")]
    pub device_sync_secs: Option<String>,
    /// `TASK_DEVICE_SYNC_ADOPT`: `1` adopts roots a device offers.
    #[architect(env = "TASK_DEVICE_SYNC_ADOPT")]
    pub device_sync_adopt: Option<String>,
    /// `TASK_FILES_COMPACT_OPS`: `0` skips the op-log compaction.
    #[architect(env = "TASK_FILES_COMPACT_OPS")]
    pub files_compact_ops: Option<String>,

    // ── forge ───────────────────────────────────────────────────────
    /// `TASK_FORGEJO_BASE_URL`.
    #[architect(env = "TASK_FORGEJO_BASE_URL")]
    pub forgejo_base_url: Option<String>,
    /// `TASK_FORGEJO_TOKEN`.
    #[architect(env = "TASK_FORGEJO_TOKEN")]
    pub forgejo_token: Option<String>,
    /// `FORGEJO_TOKEN`: the fallback for `TASK_FORGEJO_TOKEN`.
    #[architect(env = "FORGEJO_TOKEN")]
    pub forgejo_token_fallback: Option<String>,
    /// `TASK_FORGEJO_BOT_TOKEN`: the agent identity's forge token.
    #[architect(env = "TASK_FORGEJO_BOT_TOKEN")]
    pub forgejo_bot_token: Option<String>,
    /// `FTS_CODEBERG_ACCESS_TOKEN`: the fallback for the bot token (the
    /// name the sops-rendered `fts-codeberg.env` carries).
    #[architect(env = "FTS_CODEBERG_ACCESS_TOKEN")]
    pub codeberg_access_token: Option<String>,

    // ── scripture ───────────────────────────────────────────────────
    /// `TASK_ESV_API_KEY`.
    #[architect(env = "TASK_ESV_API_KEY")]
    pub esv_api_key: Option<String>,
    /// `ESV_API_KEY`: the fallback for `TASK_ESV_API_KEY`.
    #[architect(env = "ESV_API_KEY")]
    pub esv_api_key_fallback: Option<String>,
    /// `TASK_API_BIBLE_KEY`.
    #[architect(env = "TASK_API_BIBLE_KEY")]
    pub api_bible_key: Option<String>,
    /// `API_BIBLE_KEY`: the fallback for `TASK_API_BIBLE_KEY`.
    #[architect(env = "API_BIBLE_KEY")]
    pub api_bible_key_fallback: Option<String>,
    /// `TASK_API_BIBLE_NIV_ID`: the NIV edition's API.Bible id.
    #[architect(env = "TASK_API_BIBLE_NIV_ID")]
    pub api_bible_niv_id: Option<String>,
}

impl Default for ServerConfig {
    /// The configuration of an empty environment. The `server_config`
    /// test checks it equals [`ServerConfig::from_env`] with every
    /// variable unset, so the two cannot drift.
    fn default() -> Self {
        Self {
            bind: None,
            org: None,
            web_dir: None,
            build_rev: DEFAULT_BUILD_REV.to_owned(),
            auth_secret: None,
            cors_allowed_origins: None,
            enforce_permissions: None,
            enforce_media_token: None,
            public_url: None,
            share_public_base: None,
            vault_root: None,
            wiki_root: None,
            crdt_root: None,
            mail_root: None,
            agent_tasks_url: None,
            agent_runners_url: None,
            timer_url: None,
            threads_url: None,
            notify_url: None,
            prefs_url: None,
            finance_url: None,
            wiki_domains: None,
            wiki_claim_ttl_secs: None,
            wiki_repo_sync_secs: None,
            device_sync_secs: None,
            device_sync_adopt: None,
            files_compact_ops: None,
            forgejo_base_url: None,
            forgejo_token: None,
            forgejo_token_fallback: None,
            forgejo_bot_token: None,
            codeberg_access_token: None,
            esv_api_key: None,
            esv_api_key_fallback: None,
            api_bible_key: None,
            api_bible_key_fallback: None,
            api_bible_niv_id: None,
        }
    }
}

impl ServerConfig {
    /// The process environment, now.
    ///
    /// Every field is a string with no `required` and no `secret`, so
    /// the derive's `from_env` cannot fail; the fallback to
    /// [`Default`] only keeps this signature honest if a field ever
    /// gains a type that can.
    #[must_use]
    pub fn load() -> Self {
        Self::from_env().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "server config: unreadable environment, using defaults");
            Self::default()
        })
    }

    /// The listener address `main` binds: `TASK_SERVER_BIND`, else
    /// [`DEFAULT_LISTEN_BIND`].
    #[must_use]
    pub fn listen_bind(&self) -> &str {
        self.bind.as_deref().unwrap_or(DEFAULT_LISTEN_BIND)
    }

    /// The bind address a share link falls back to without a public
    /// base: `TASK_SERVER_BIND`, else [`DEFAULT_SHARE_BIND`].
    #[must_use]
    pub fn share_bind(&self) -> &str {
        self.bind.as_deref().unwrap_or(DEFAULT_SHARE_BIND)
    }

    /// `TASK_SHARE_PUBLIC_BASE`, else `TASK_SERVER_PUBLIC_URL`; `None`
    /// when neither is set.
    #[must_use]
    pub fn share_public_base_explicit(&self) -> Option<&str> {
        self.share_public_base
            .as_deref()
            .or(self.public_url.as_deref())
    }

    /// The forge token: `TASK_FORGEJO_TOKEN`, else `FORGEJO_TOKEN`, else
    /// empty (the forge methods then answer an auth error).
    #[must_use]
    pub fn forgejo_token(&self) -> &str {
        self.forgejo_token
            .as_deref()
            .or(self.forgejo_token_fallback.as_deref())
            .unwrap_or_default()
    }

    /// The agent's forge token: `TASK_FORGEJO_BOT_TOKEN`, else
    /// `FTS_CODEBERG_ACCESS_TOKEN`.
    #[must_use]
    pub fn forgejo_bot_token(&self) -> Option<&str> {
        self.forgejo_bot_token
            .as_deref()
            .or(self.codeberg_access_token.as_deref())
    }

    /// The ESV key: `TASK_ESV_API_KEY`, else `ESV_API_KEY`.
    #[must_use]
    pub fn esv_api_key(&self) -> Option<&str> {
        self.esv_api_key
            .as_deref()
            .or(self.esv_api_key_fallback.as_deref())
    }

    /// The API.Bible key: `TASK_API_BIBLE_KEY`, else `API_BIBLE_KEY`.
    #[must_use]
    pub fn api_bible_key(&self) -> Option<&str> {
        self.api_bible_key
            .as_deref()
            .or(self.api_bible_key_fallback.as_deref())
    }

    /// Whether `TASK_ENFORCE_PERMISSIONS` is exactly `1`.
    #[must_use]
    pub fn enforces_permissions(&self) -> bool {
        self.enforce_permissions.as_deref() == Some("1")
    }

    /// Whether `TASK_ENFORCE_MEDIA_TOKEN` is exactly `1`.
    #[must_use]
    pub fn enforces_media_token(&self) -> bool {
        self.enforce_media_token.as_deref() == Some("1")
    }

    /// Whether `TASK_DEVICE_SYNC_ADOPT` is exactly `1`.
    #[must_use]
    pub fn adopts_offered_roots(&self) -> bool {
        self.device_sync_adopt.as_deref() == Some("1")
    }

    /// Whether to compact op logs before the first device sweep: on
    /// unless `TASK_FILES_COMPACT_OPS` is `0`.
    #[must_use]
    pub fn compacts_op_logs(&self) -> bool {
        self.files_compact_ops.as_deref() != Some("0")
    }

    /// `TASK_WIKI_CLAIM_TTL_SECS` as seconds, when it parses.
    #[must_use]
    pub fn wiki_claim_ttl(&self) -> Option<std::time::Duration> {
        self.wiki_claim_ttl_secs
            .as_deref()
            .and_then(|v| v.parse::<u64>().ok())
            .map(std::time::Duration::from_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_bind_defaults_stay_apart() {
        let unset = ServerConfig::default();
        assert_eq!(unset.listen_bind(), "0.0.0.0:9090");
        assert_eq!(unset.share_bind(), "127.0.0.1:3456");
        let set = ServerConfig {
            bind: Some("10.0.0.1:1".into()),
            ..ServerConfig::default()
        };
        assert_eq!(set.listen_bind(), "10.0.0.1:1");
        assert_eq!(set.share_bind(), "10.0.0.1:1");
    }

    #[test]
    fn prefixed_names_win_over_their_fallbacks() {
        let both = ServerConfig {
            forgejo_token: Some("task".into()),
            forgejo_token_fallback: Some("plain".into()),
            forgejo_bot_token: Some("bot".into()),
            codeberg_access_token: Some("codeberg".into()),
            esv_api_key: Some("task-esv".into()),
            esv_api_key_fallback: Some("esv".into()),
            api_bible_key: Some("task-bible".into()),
            api_bible_key_fallback: Some("bible".into()),
            share_public_base: Some("https://share".into()),
            public_url: Some("https://public".into()),
            ..ServerConfig::default()
        };
        assert_eq!(both.forgejo_token(), "task");
        assert_eq!(both.forgejo_bot_token(), Some("bot"));
        assert_eq!(both.esv_api_key(), Some("task-esv"));
        assert_eq!(both.api_bible_key(), Some("task-bible"));
        assert_eq!(both.share_public_base_explicit(), Some("https://share"));

        let fallbacks = ServerConfig {
            forgejo_token: None,
            forgejo_bot_token: None,
            esv_api_key: None,
            api_bible_key: None,
            share_public_base: None,
            ..both
        };
        assert_eq!(fallbacks.forgejo_token(), "plain");
        assert_eq!(fallbacks.forgejo_bot_token(), Some("codeberg"));
        assert_eq!(fallbacks.esv_api_key(), Some("esv"));
        assert_eq!(fallbacks.api_bible_key(), Some("bible"));
        assert_eq!(
            fallbacks.share_public_base_explicit(),
            Some("https://public")
        );
        assert_eq!(ServerConfig::default().forgejo_token(), "");
    }

    #[test]
    fn switches_need_their_exact_value() {
        let on = ServerConfig {
            enforce_permissions: Some("1".into()),
            enforce_media_token: Some("1".into()),
            files_compact_ops: Some("0".into()),
            ..ServerConfig::default()
        };
        assert!(on.enforces_permissions());
        assert!(on.enforces_media_token());
        assert!(!on.compacts_op_logs());
        let near = ServerConfig {
            enforce_permissions: Some("true".into()),
            enforce_media_token: Some("yes".into()),
            files_compact_ops: Some("false".into()),
            ..ServerConfig::default()
        };
        assert!(!near.enforces_permissions());
        assert!(!near.enforces_media_token());
        assert!(near.compacts_op_logs());
        let unset = ServerConfig::default();
        assert!(!unset.enforces_permissions());
        assert!(!unset.enforces_media_token());
        assert!(unset.compacts_op_logs());
    }

    #[test]
    fn a_claim_ttl_that_does_not_parse_is_ignored() {
        let ttl = |v: &str| {
            ServerConfig {
                wiki_claim_ttl_secs: Some(v.into()),
                ..ServerConfig::default()
            }
            .wiki_claim_ttl()
        };
        assert_eq!(ttl("90"), Some(std::time::Duration::from_secs(90)));
        assert_eq!(ttl("soon"), None);
        assert_eq!(ServerConfig::default().wiki_claim_ttl(), None);
    }
}
