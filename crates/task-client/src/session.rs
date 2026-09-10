//! The persistent session: who this machine is signed in as, and
//! where.
//!
//! Written by `task auth login` / `task auth signup`, read by anything
//! that needs an authenticated `user_id` or a server to talk to. Lives
//! at `$XDG_DATA_HOME/task/session.json` (override via
//! `TASK_SESSION_FILE`).
//!
//! ## The one trap in this module — read this before hand-writing a
//! session file
//!
//! **`session.json` is a routing document. It does not contain the
//! token.** The token for session key `k` lives in a *sibling*
//! `<stem>-tokens/<k>.json`, written through architect-auth's
//! [`FileTokenStore`] so it gets an atomic write and `0600`. The
//! routing document holds only `home` / `active` / `key → {url, slug}`.
//!
//! This split has a failure mode that is worth a paragraph because it
//! costs an afternoon every time somebody meets it. Write a session
//! file by hand with the token inline — the obvious shape, and the
//! shape every older version of this file used — and the token is
//! simply not where anything looks for it. Before this crate existed,
//! [`load`] responded by *silently dropping the whole entry*, on the
//! reasoning that a missing token file means "signed out
//! out-of-band". The result: the file parses, `task auth whoami`
//! prints the account, and every RPC goes out with no `Authorization`
//! header at all. The server then answers `anonymous is not a member`,
//! which reads exactly like a permissions problem and is not one. The
//! account is fine. The membership is fine. The token never left the
//! disk.
//!
//! Two changes make that unreachable rather than merely documented:
//!
//! 1. **An inline token is adopted, not ignored.** If a key has no
//!    token file but the routing entry carries `token` + `user_id`,
//!    [`load`] promotes it into a proper token file and re-saves. This
//!    is also what finally makes the pre-split upgrade path work: an
//!    old embedded-token file whose entries had a `slug` field parsed
//!    as the *current* shape, so the legacy branch below never ran and
//!    the tokens went in the bin.
//! 2. **A half-written entry is loud.** An inline `token` with no
//!    `user_id` cannot be adopted (a session needs the id), so it is an
//!    error naming the key, the token path, and the rule — instead of a
//!    silent drop that surfaces three layers away as an authorization
//!    failure.
//!
//! A key that genuinely has nothing — no token file, no inline token —
//! is still dropped, because that really is the signed-out-out-of-band
//! case. It is now *reported*: [`load_report`] returns what was
//! dropped so a caller can say so, and [`load`] stays the terse form
//! for callers that do not care.
//!
//! ## Shape — multi-server, server-aware
//!
//! `home` is the identity-anchor entry (the user's personal org).
//! `active` is the entry subsequent commands run against. `servers`
//! is a map keyed by **session key** — `entry_key(slug, url)` — so
//! the same org slug signed into two different servers (local dev +
//! production) coexists as two independent entries with two
//! independent tokens. Each entry records the org slug AND the
//! server vox base URL it was issued by, which is how an
//! environment-variable URL switch (`TASK_VOX_URL`) finds the right
//! signed-in session automatically (see `entry_for_server`).
//!
//! ## Storage
//!
//! Bearer tokens are persisted through architect-auth's client kit
//! ([`FileTokenStore`]) — atomic temp-file + rename writes, `0600` on
//! unix — one `StoredSession` JSON per session key under a
//! `…-tokens/` directory next to the session file. `session.json`
//! itself is reduced to the non-secret routing document (`home` /
//! `active` / key → `{url, slug}` map).
//!
//! ## Back-compat
//!
//! [`load`] silently upgrades the older on-disk shapes — the
//! pre-PR-2 single-org `{token, user_id, email, org_id}`, the
//! combined multi-server document that embedded tokens directly in
//! `session.json`, and the slug-keyed routing doc whose values were
//! plain URL strings — to the current layout. Legacy entries keyed
//! by bare slug with `url: "local"` keep working: `"local"` is
//! treated as "whatever loopback server / on-disk data root this
//! machine runs" for matching purposes.

use std::collections::BTreeMap;
use std::path::PathBuf;

use architect_auth::client::{FileTokenStore, StoredSession, TokenStore as _};
use serde::{Deserialize, Serialize};

/// Default local server base (the dev `task-server`).
pub const DEFAULT_LOCAL_VOX: &str = "ws://127.0.0.1:18080";

/// Sentinel URL for orgs served from the local data root (legacy
/// entries, plus sqlite-direct flows that never touched a server).
pub const LOCAL_URL: &str = "local";

/// Per-org server entry. One row per (server-the-CLI-has-signed-into
/// × org-it-targets). Local-dev and production sessions for the same
/// slug coexist as separate entries (separate session keys, separate
/// token files).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerEntry {
    /// Where this org lives: a vox base URL
    /// (`wss://task.starcommand.live`) or [`LOCAL_URL`] for an org
    /// served by the local `task-server` / opened directly via
    /// SQLite by the CLI.
    pub url: String,
    /// The org slug on that server. Distinct from the map key —
    /// the key may carry a server suffix (`slug@host`).
    #[serde(default)]
    pub slug: String,
    /// Authenticated user id (from architect-auth) within this org.
    pub user_id: uuid::Uuid,
    /// Email captured at sign-in. Cached for `task auth whoami`;
    /// not used for routing.
    pub email: String,
    /// Bearer token for the active architect-auth session. Stored
    /// via [`FileTokenStore`] (atomic write, `0600`), never in the
    /// routing document.
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliSession {
    /// Identity anchor — the user's personal/home entry key. Empty
    /// until first login from a home server.
    #[serde(default)]
    pub home: String,
    /// Currently-active entry key. Subsequent commands operate
    /// against this entry's slug + server.
    pub active: String,
    /// Session key → server entry. Populated by `task auth login`
    /// per signed-into (server, org) pair.
    pub servers: BTreeMap<String, ServerEntry>,
}

/// Routing-doc value: where an entry lives. Older files stored a
/// plain URL string keyed by slug; current files store `{url, slug}`
/// keyed by session key. Deserialize accepts both; serialize always
/// emits the struct form.
///
/// The three trailing fields are **read-only**: they are never
/// serialized (see the `skip_serializing_if`s), and they exist purely
/// so [`load_at`] can see a token that somebody — a previous version
/// of this program, or a person editing the file — put inline. Without
/// them serde parses such a document happily and discards exactly the
/// field that matters. See the module doc.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum RouteVal {
    Meta {
        url: String,
        slug: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
    },
    Url(String),
}

/// A session key that was present in the routing document but produced
/// no usable entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    /// The session key, e.g. `codywright@task.starcommand.live`.
    pub key: String,
    /// Where its token was looked for.
    pub token_file: PathBuf,
}

/// The full result of reading a session: what loaded, and what did
/// not.
///
/// [`load`] throws the second half away. Anything user-facing should
/// not: a dropped key is the difference between "you are signed out"
/// and "you are signed in and every request is anonymous", and only
/// this struct can tell them apart.
#[derive(Debug, Default)]
pub struct Load {
    pub session: Option<CliSession>,
    pub dropped: Vec<Dropped>,
}

/// What `session.json` holds on disk: the non-secret routing state.
/// Tokens (plus the cached user id / email) live in per-key
/// [`FileTokenStore`] files. Don't reference outside this module.
#[derive(Debug, Serialize, Deserialize)]
struct RoutingDoc {
    #[serde(default)]
    home: String,
    active: String,
    /// Session key → routing value. The rest of the entry is
    /// rebuilt from the key's token file.
    servers: BTreeMap<String, RouteVal>,
}

/// Pre-split multi-server shape: tokens embedded directly in
/// `session.json`. Deserialize-only shim so [`load`] can upgrade in
/// place. Don't reference outside this module.
#[derive(Debug, Deserialize)]
struct EmbeddedSession {
    #[serde(default)]
    home: String,
    active: String,
    servers: BTreeMap<String, ServerEntry>,
}

/// Pre-PR-2 single-org shape. Kept as a deserialize-only shim so
/// [`load`] can upgrade legacy session files in place. Don't
/// reference outside this module. `org_id` here was the
/// architect-auth membership id — different concept from the on-disk
/// org slug; we drop it on upgrade and let `task auth org use`
/// re-set it.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct LegacySession {
    token: String,
    user_id: uuid::Uuid,
    email: String,
    org_id: Option<uuid::Uuid>,
}

// ── server-url helpers ──────────────────────────────────────────────

/// Normalize a server reference to a comparable vox base URL:
/// `"local"` / empty → [`DEFAULT_LOCAL_VOX`]; `http(s)://` →
/// `ws(s)://`; scheme-less input gets `ws://`; trailing `/` and the
/// `/vox` per-org-hint suffix are stripped. The per-org endpoint is
/// always `<base>/org/<slug>/vox`.
#[must_use]
pub fn normalize_server_base(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() || t == LOCAL_URL {
        return DEFAULT_LOCAL_VOX.to_owned();
    }
    let ws: String = if let Some(rest) = t.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = t.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if t.starts_with("ws://") || t.starts_with("wss://") {
        t.to_owned()
    } else {
        format!("ws://{t}")
    };
    ws.trim_end_matches('/')
        .trim_end_matches("/vox")
        .trim_end_matches('/')
        .to_owned()
}

/// True when the normalized base points at this machine's loopback.
#[must_use]
fn is_loopback_base(base: &str) -> bool {
    let b = normalize_server_base(base);
    let Some(rest) = b.strip_prefix("ws://") else {
        return false;
    };
    rest.starts_with("127.0.0.1") || rest.starts_with("localhost")
}

/// Do two server references address the same server? Exact
/// normalized match, plus the legacy wildcard: a stored literal
/// `"local"` matches any loopback target (the local data root is
/// shared by every locally-run server, whatever its port).
#[must_use]
pub fn same_server(a: &str, b: &str) -> bool {
    if normalize_server_base(a) == normalize_server_base(b) {
        return true;
    }
    (a.trim() == LOCAL_URL && is_loopback_base(b)) || (b.trim() == LOCAL_URL && is_loopback_base(a))
}

/// Session key for a (slug, server) pair. Local entries keep the
/// bare slug (back-compat with existing session files); remote
/// entries append the server host so the same slug on two servers
/// never collides: `codywright@task.starcommand.live`. The key
/// doubles as the token file stem, so it stays filesystem-safe
/// (no `/`).
#[must_use]
pub fn entry_key(slug: &str, url: &str) -> String {
    if url.trim() == LOCAL_URL || is_loopback_base(url) {
        return slug.to_owned();
    }
    let base = normalize_server_base(url);
    let host = base
        .trim_start_matches("wss://")
        .trim_start_matches("ws://")
        .replace('/', "_");
    format!("{slug}@{host}")
}

// ── paths ───────────────────────────────────────────────────────────

/// The two on-disk locations a session occupies, resolved once so
/// every read/write in one operation agrees, and so tests can point
/// the store at a temp dir without touching process env.
#[derive(Debug, Clone)]
pub struct Paths {
    session: PathBuf,
    tokens: PathBuf,
}

impl Paths {
    /// Derive the token dir from a session file path:
    /// `session.json` → `session-tokens/`. The `TASK_SESSION_FILE`
    /// override moves both together.
    #[must_use]
    pub fn for_session_file(session: PathBuf) -> Self {
        let stem = session.file_stem().map_or_else(
            || "session".to_owned(),
            |s| s.to_string_lossy().into_owned(),
        );
        let tokens = session.with_file_name(format!("{stem}-tokens"));
        Self { session, tokens }
    }
}

/// Resolve the session path. `$TASK_SESSION_FILE` wins; else
/// `$XDG_DATA_HOME/task/session.json` with the standard
/// `$HOME/.local/share` fallback. Creates parent dirs.
pub fn session_path() -> eyre::Result<PathBuf> {
    if let Ok(explicit) = std::env::var("TASK_SESSION_FILE") {
        if !explicit.is_empty() {
            let p = PathBuf::from(explicit);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| eyre::eyre!("create {}: {e}", parent.display()))?;
            }
            return Ok(p);
        }
    }
    let base = match std::env::var("XDG_DATA_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            let home = std::env::var("HOME")
                .map_err(|_| eyre::eyre!("neither XDG_DATA_HOME nor HOME is set"))?;
            PathBuf::from(home).join(".local").join("share")
        }
    };
    let dir = base.join("task");
    std::fs::create_dir_all(&dir).map_err(|e| eyre::eyre!("create {}: {e}", dir.display()))?;
    Ok(dir.join("session.json"))
}

fn paths() -> eyre::Result<Paths> {
    Ok(Paths::for_session_file(session_path()?))
}

/// The [`FileTokenStore`] for one session key.
fn token_store(p: &Paths, key: &str) -> FileTokenStore {
    FileTokenStore::new(p.tokens.join(format!("{key}.json")))
}

fn entry_to_stored(entry: &ServerEntry) -> StoredSession {
    StoredSession::new(entry.token.clone())
        .with_user_id(entry.user_id.to_string())
        .with_email(entry.email.clone())
}

fn entry_from_stored(
    key: &str,
    url: String,
    slug: String,
    stored: StoredSession,
) -> eyre::Result<ServerEntry> {
    let user_id = stored
        .user_id
        .as_deref()
        .ok_or_else(|| eyre::eyre!("token file for `{key}` has no user_id"))?;
    let user_id = user_id
        .parse::<uuid::Uuid>()
        .map_err(|e| eyre::eyre!("token file for `{key}`: bad user_id `{user_id}`: {e}"))?;
    Ok(ServerEntry {
        url,
        slug,
        user_id,
        email: stored.email.unwrap_or_default(),
        token: stored.token,
    })
}

// ── load / save / clear ─────────────────────────────────────────────

/// Read the session, discarding the report of what could not be read.
/// The convenient form; [`load_report`] is the honest one.
pub fn load() -> eyre::Result<Option<CliSession>> {
    Ok(load_report()?.session)
}

/// Read the session **and** report every key that was skipped. Prefer
/// this anywhere the answer is shown to a person: see the module doc
/// on why a silently-skipped key looks like a permissions failure.
pub fn load_report() -> eyre::Result<Load> {
    load_at(&paths()?)
}

pub fn load_at(p: &Paths) -> eyre::Result<Load> {
    let path = &p.session;
    if !path.exists() {
        return Ok(Load::default());
    }
    let raw =
        std::fs::read_to_string(path).map_err(|e| eyre::eyre!("read {}: {e}", path.display()))?;
    // Current shape: routing doc + per-key token files.
    if let Ok(doc) = serde_json::from_str::<RoutingDoc>(&raw) {
        let mut servers = BTreeMap::new();
        let mut dropped = Vec::new();
        // Set when an inline token had to be promoted into a token
        // file, so the repaired shape is written back once and the
        // wrong shape is never tolerated twice.
        let mut repaired = false;
        for (key, val) in doc.servers {
            let (url, slug, inline) = match val {
                RouteVal::Meta {
                    url,
                    slug,
                    token,
                    user_id,
                    email,
                } => (url, slug, Some((token, user_id, email))),
                // Slug-keyed legacy row: the key WAS the slug.
                RouteVal::Url(url) => (url, key.clone(), None),
            };
            let stored = token_store(p, &key)
                .load()
                .map_err(|e| eyre::eyre!("load token for `{key}`: {e}"))?;
            let stored = match (stored, inline) {
                // The ordinary path: the token file is authoritative.
                // An inline token alongside it is stale routing-doc
                // residue and is ignored, deliberately — the file with
                // `0600` on it wins over the one without.
                (Some(stored), _) => stored,
                // No token file, but the routing entry carries a whole
                // credential: adopt it. This is the pre-split shape,
                // and adopting is the upgrade that shape was always
                // supposed to get.
                (None, Some((Some(token), Some(user_id), email))) if !token.is_empty() => {
                    repaired = true;
                    StoredSession::new(token)
                        .with_user_id(user_id.to_string())
                        .with_email(email.unwrap_or_default())
                }
                // A token with no user id cannot become a session, and
                // pretending otherwise is exactly the silent-anonymous
                // failure this module exists to prevent. Say so, name
                // the file, and state the rule.
                (None, Some((Some(_), None, _))) => {
                    return Err(eyre::eyre!(
                        "session entry `{key}` has an inline `token` but no `user_id`, so it \
                         cannot be adopted.\n  {} is a ROUTING document: the token belongs in \
                         {}.\n  Left as-is every request would go out with no Authorization \
                         header and the server would answer `anonymous is not a member` — a \
                         failure that reads like a permissions problem and is not one.\n  Fix: \
                         add `user_id`, or re-run `task auth login`.",
                        path.display(),
                        p.tokens.join(format!("{key}.json")).display(),
                    ));
                }
                // Nothing anywhere: genuinely signed out out-of-band.
                // Drop the entry rather than failing every command —
                // but say that we did.
                (None, _) => {
                    dropped.push(Dropped {
                        key: key.clone(),
                        token_file: p.tokens.join(format!("{key}.json")),
                    });
                    continue;
                }
            };
            servers.insert(key.clone(), entry_from_stored(&key, url, slug, stored)?);
        }
        let sess = CliSession {
            home: doc.home,
            active: doc.active,
            servers,
        };
        if repaired {
            save_at(p, &sess)?;
        }
        return Ok(Load {
            session: Some(sess),
            dropped,
        });
    }
    // Pre-split multi-server shape (tokens embedded in
    // session.json): upgrade to the split layout. Keys were slugs.
    if let Ok(embedded) = serde_json::from_str::<EmbeddedSession>(&raw) {
        let servers = embedded
            .servers
            .into_iter()
            .map(|(key, mut entry)| {
                if entry.slug.is_empty() {
                    entry.slug = key.clone();
                }
                (key, entry)
            })
            .collect();
        let sess = CliSession {
            home: embedded.home,
            active: embedded.active,
            servers,
        };
        save_at(p, &sess)?;
        return Ok(Load {
            session: Some(sess),
            dropped: Vec::new(),
        });
    }
    // Fall back to legacy single-org shape and upgrade.
    let legacy: LegacySession = serde_json::from_str(&raw).map_err(|e| {
        eyre::eyre!(
            "parse {} (neither new nor legacy shape): {e}",
            path.display()
        )
    })?;
    let slug = "default".to_owned();
    let mut servers = BTreeMap::new();
    servers.insert(
        slug.clone(),
        ServerEntry {
            url: LOCAL_URL.into(),
            slug: slug.clone(),
            user_id: legacy.user_id,
            email: legacy.email,
            token: legacy.token,
        },
    );
    let sess = CliSession {
        home: slug.clone(),
        active: slug,
        servers,
    };
    // Persist the upgraded shape so the legacy form is only ever
    // tolerated once.
    save_at(p, &sess)?;
    Ok(Load {
        session: Some(sess),
        dropped: Vec::new(),
    })
}

pub fn save(sess: &CliSession) -> eyre::Result<()> {
    save_at(&paths()?, sess)
}

pub fn save_at(p: &Paths, sess: &CliSession) -> eyre::Result<()> {
    let path = &p.session;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| eyre::eyre!("create {}: {e}", parent.display()))?;
    }
    let dir = &p.tokens;
    std::fs::create_dir_all(dir).map_err(|e| eyre::eyre!("create {}: {e}", dir.display()))?;
    // Tokens first — one FileTokenStore (atomic write, 0600) per key.
    for (key, entry) in &sess.servers {
        token_store(p, key)
            .save(&entry_to_stored(entry))
            .map_err(|e| eyre::eyre!("save token for `{key}`: {e}"))?;
    }
    // Prune token files for keys no longer in the session (logout
    // removes the entry then calls `save`).
    for dirent in std::fs::read_dir(dir).map_err(|e| eyre::eyre!("read {}: {e}", dir.display()))? {
        let dirent = dirent.map_err(|e| eyre::eyre!("read {}: {e}", dir.display()))?;
        let name = dirent.file_name();
        let Some(key) = name
            .to_string_lossy()
            .strip_suffix(".json")
            .map(str::to_owned)
        else {
            continue;
        };
        if !sess.servers.contains_key(&key) {
            token_store(p, &key)
                .clear()
                .map_err(|e| eyre::eyre!("clear token for `{key}`: {e}"))?;
        }
    }
    // Then the non-secret routing doc, atomically (temp + rename)
    // so a crash never leaves a truncated file.
    let doc = RoutingDoc {
        home: sess.home.clone(),
        active: sess.active.clone(),
        servers: sess
            .servers
            .iter()
            .map(|(key, entry)| {
                (
                    key.clone(),
                    RouteVal::Meta {
                        url: entry.url.clone(),
                        slug: if entry.slug.is_empty() {
                            key.clone()
                        } else {
                            entry.slug.clone()
                        },
                        // Never written back. The token went to its own
                        // `0600` file above; putting a copy here would
                        // recreate the very shape the module doc warns
                        // about, in a world-readable file.
                        token: None,
                        user_id: None,
                        email: None,
                    },
                )
            })
            .collect(),
    };
    let raw =
        serde_json::to_string_pretty(&doc).map_err(|e| eyre::eyre!("serialize session: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, raw).map_err(|e| eyre::eyre!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| eyre::eyre!("rename {} -> {}: {e}", tmp.display(), path.display()))?;
    Ok(())
}

pub fn clear() -> eyre::Result<()> {
    clear_at(&paths()?)
}

pub fn clear_at(p: &Paths) -> eyre::Result<()> {
    let path = &p.session;
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| eyre::eyre!("remove {}: {e}", path.display()))?;
    }
    match std::fs::remove_dir_all(&p.tokens) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(eyre::eyre!("remove {}: {e}", p.tokens.display())),
    }
    Ok(())
}

// ── session-level helpers ───────────────────────────────────────────

impl CliSession {
    /// Lookup helper. `None` if the active key has no entry (e.g. a
    /// stale `active` after the entry was removed).
    #[must_use]
    pub fn active_server(&self) -> Option<&ServerEntry> {
        self.servers.get(&self.active)
    }

    /// The **home** entry — the identity anchor, and the only server
    /// that holds this account's identity locker. Distinct from
    /// [`Self::active_server`]: you can be working in a linked org
    /// while home stays where your links live.
    #[must_use]
    pub fn home_entry(&self) -> Option<&ServerEntry> {
        self.servers
            .get(&self.home)
            // A session predating `home` (or one whose home entry was
            // removed) still has an active server; treating that as
            // home beats refusing to work at all.
            .or_else(|| self.active_server())
    }

    /// The active entry's org slug. Falls back to the raw key for
    /// stale sessions (pre-server-aware keys WERE slugs).
    #[must_use]
    pub fn active_slug(&self) -> String {
        self.active_server().map_or_else(
            || self.active.clone(),
            |e| {
                if e.slug.is_empty() {
                    self.active.clone()
                } else {
                    e.slug.clone()
                }
            },
        )
    }

    /// Find the signed-in entry for a target server (the
    /// `--server` / `TASK_VOX_URL` switch following its session
    /// automatically). Preference among matches: the active entry,
    /// then the home entry (the identity anchor — what `active`
    /// pointed at before a remote login moved it), then the first
    /// matching entry in key order.
    #[must_use]
    pub fn entry_for_server(&self, server: &str) -> Option<(&str, &ServerEntry)> {
        for key in [&self.active, &self.home] {
            if let Some(entry) = self.servers.get(key) {
                if same_server(&entry.url, server) {
                    return Some((key.as_str(), entry));
                }
            }
        }
        self.servers
            .iter()
            .find(|(_, e)| same_server(&e.url, server))
            .map(|(k, e)| (k.as_str(), e))
    }

    /// Insert/update the entry for a fresh sign-in and make it
    /// active (`home` defaults to the first server signed into —
    /// the personal-org-as-home pattern). Returns the session key.
    pub fn record_login(
        &mut self,
        slug: &str,
        url: &str,
        user_id: uuid::Uuid,
        email: String,
        token: String,
    ) -> String {
        let key = entry_key(slug, url);
        self.servers.insert(
            key.clone(),
            ServerEntry {
                url: url.to_owned(),
                slug: slug.to_owned(),
                user_id,
                email,
                token,
            },
        );
        self.active = key.clone();
        if self.home.is_empty() {
            self.home = key.clone();
        }
        key
    }

    /// An empty session to grow a first login into.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            home: String::new(),
            active: String::new(),
            servers: BTreeMap::new(),
        }
    }
}

/// Auth secret. Must match `task-server`'s `DEFAULT_AUTH_SECRET`
/// since both processes hash + verify session tokens against the
/// same value.
pub const DEFAULT_AUTH_SECRET: &str = "task-server-auth-dev-secret-32+!";

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(tag: &str) -> Paths {
        let dir = std::env::temp_dir().join(format!(
            "task-session-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Paths::for_session_file(dir.join("session.json"))
    }

    #[test]
    fn normalize_server_base_shapes() {
        assert_eq!(normalize_server_base("local"), DEFAULT_LOCAL_VOX);
        assert_eq!(normalize_server_base(""), DEFAULT_LOCAL_VOX);
        assert_eq!(
            normalize_server_base("ws://127.0.0.1:18080/vox"),
            "ws://127.0.0.1:18080"
        );
        assert_eq!(
            normalize_server_base("wss://task.starcommand.live/vox"),
            "wss://task.starcommand.live"
        );
        assert_eq!(
            normalize_server_base("https://task.starcommand.live"),
            "wss://task.starcommand.live"
        );
        assert_eq!(
            normalize_server_base("task.starcommand.live"),
            "ws://task.starcommand.live"
        );
        assert_eq!(
            normalize_server_base("ws://127.0.0.1:18080/"),
            "ws://127.0.0.1:18080"
        );
    }

    #[test]
    fn same_server_matches_local_wildcard() {
        assert!(same_server("local", "ws://127.0.0.1:18080/vox"));
        assert!(same_server("ws://localhost:9090", "local"));
        assert!(!same_server("local", "wss://task.starcommand.live/vox"));
        assert!(same_server(
            "wss://task.starcommand.live/vox",
            "https://task.starcommand.live"
        ));
        // Two distinct loopback ports are NOT the same server —
        // only the literal "local" sentinel wildcards.
        assert!(!same_server("ws://127.0.0.1:18080", "ws://127.0.0.1:18100"));
    }

    #[test]
    fn entry_keys_split_by_server() {
        assert_eq!(entry_key("fts", "local"), "fts");
        assert_eq!(entry_key("fts", "ws://127.0.0.1:18080"), "fts");
        assert_eq!(
            entry_key("codywright", "wss://task.starcommand.live/vox"),
            "codywright@task.starcommand.live"
        );
        assert!(!entry_key("a", "wss://h/x/y").contains('/'));
    }

    #[test]
    fn session_round_trips_with_server_meta() {
        let p = temp_paths("roundtrip");
        let mut sess = CliSession::empty();
        let uid = uuid::Uuid::new_v4();
        sess.record_login("fts", LOCAL_URL, uid, "a@x".into(), "tok-local".into());
        let key = sess.record_login(
            "codywright",
            "wss://task.starcommand.live/vox",
            uid,
            "cody@x".into(),
            "tok-prod".into(),
        );
        assert_eq!(key, "codywright@task.starcommand.live");
        assert_eq!(sess.active, key);
        assert_eq!(sess.home, "fts");
        save_at(&p, &sess).unwrap();

        let loaded = load_at(&p).unwrap().session.expect("session exists");
        assert_eq!(loaded.active, key);
        assert_eq!(loaded.active_slug(), "codywright");
        let prod = loaded.servers.get(&key).unwrap();
        assert_eq!(prod.url, "wss://task.starcommand.live/vox");
        assert_eq!(prod.token, "tok-prod");
        assert_eq!(prod.email, "cody@x");
        let local = loaded.servers.get("fts").unwrap();
        assert_eq!(local.token, "tok-local");
        // Local + prod coexist as distinct token files.
        assert_eq!(loaded.servers.len(), 2);
        clear_at(&p).unwrap();
        assert!(load_at(&p).unwrap().session.is_none());
    }

    #[test]
    fn entry_for_server_follows_url_switch() {
        let mut sess = CliSession::empty();
        let uid = uuid::Uuid::new_v4();
        sess.record_login("fts", LOCAL_URL, uid, "a@x".into(), "t1".into());
        sess.record_login(
            "codywright",
            "wss://task.starcommand.live/vox",
            uid,
            "c@x".into(),
            "t2".into(),
        );
        // Active is prod; an explicit prod URL resolves to it.
        let (k, e) = sess
            .entry_for_server("wss://task.starcommand.live/vox")
            .unwrap();
        assert_eq!(k, "codywright@task.starcommand.live");
        assert_eq!(e.slug, "codywright");
        // Flipping TASK_VOX_URL to the local dev server finds the
        // local entry even though prod is active.
        let (k, e) = sess.entry_for_server("ws://127.0.0.1:18080/vox").unwrap();
        assert_eq!(k, "fts");
        assert_eq!(e.slug, "fts");
        // With several local entries, the home anchor wins over
        // BTreeMap key order.
        sess.record_login("aaa", LOCAL_URL, uid, "z@x".into(), "t3".into());
        sess.active = "codywright@task.starcommand.live".into();
        assert_eq!(sess.home, "fts");
        let (k, _) = sess.entry_for_server("ws://127.0.0.1:18080/vox").unwrap();
        assert_eq!(k, "fts", "home preferred over alphabetical `aaa`");
        // Unknown server: no entry.
        assert!(sess.entry_for_server("wss://elsewhere.example").is_none());
    }

    #[test]
    fn legacy_slug_keyed_routing_doc_upgrades() {
        let p = temp_paths("legacy-routing");
        // Old shape: values are plain URL strings, keys are slugs.
        std::fs::create_dir_all(p.session.parent().unwrap()).unwrap();
        std::fs::write(
            &p.session,
            r#"{ "home": "fts", "active": "fts", "servers": { "fts": "local" } }"#,
        )
        .unwrap();
        std::fs::create_dir_all(&p.tokens).unwrap();
        let uid = uuid::Uuid::new_v4();
        token_store(&p, "fts")
            .save(
                &StoredSession::new("tok".to_owned())
                    .with_user_id(uid.to_string())
                    .with_email("a@x".to_owned()),
            )
            .unwrap();
        let loaded = load_at(&p).unwrap().session.expect("session exists");
        let e = loaded.servers.get("fts").unwrap();
        assert_eq!(e.slug, "fts", "slug recovered from the legacy key");
        assert_eq!(e.url, "local");
        assert_eq!(loaded.active_slug(), "fts");
        clear_at(&p).unwrap();
    }
}
