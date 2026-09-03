//! Share links — notes, Root slices, and Named Versions (issue #271).
//!
//! [`ShareStore`] persists an org's links in `<org>/shares.json` (a flat,
//! human-inspectable file — the sqlite move comes with grants/audiences)
//! and the per-link access log in `<org>/shares-access.jsonl`;
//! [`ShareServiceImpl`] serves link CRUD on the ORG lane; the HTTP
//! routes here (`GET /org/{slug}/share/{token}[/…]`, wired in `lib.rs`)
//! resolve a token on EVERY hit — 404 unknown, 410 disabled/expired,
//! password re-checked — so every setting is retroactive by
//! construction.
//!
//! ## The Files targets (issue #271)
//!
//! A **Slice** link serves exactly its `(root, subpath)` subtree: the
//! link's URLs carry paths *relative to the slice*, so nothing outside
//! it is even addressable (AC 1). A **Named Version** link resolves the
//! curated entity to its exact change and serves *that* commit's tree
//! (AC 2), whatever the live tree looks like today. View-only links
//! stream proxy renditions and never original bytes; the `download`
//! capability gates originals and every download writes a receipt into
//! the access log (AC 3/4).

use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::{Path as AxPath, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use files::FilesService as _;
use futures_util::TryStreamExt as _;
use sha2::{Digest, Sha256};
use share_proto::{
    NewShareLink, ShareAccess, ShareCapabilities, ShareError, ShareLinkInfo, ShareService,
    ShareTarget,
};

use crate::AppState;

// ── storage ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StoredLink {
    pub token: String,
    pub label: String,
    /// `None` only on rows written before issue #271 — those were all
    /// note links; [`StoredLink::target`] reconstructs them.
    #[serde(default)]
    pub target: Option<ShareTarget>,
    /// Legacy pre-#271 fields, kept so an existing `shares.json` loads.
    #[serde(default)]
    pub note_path: String,
    #[serde(default)]
    pub capability: String,
    #[serde(default)]
    pub capabilities: Option<ShareCapabilities>,
    /// SHA-256 hex of the link password; `None` = open.
    #[serde(default)]
    pub password_sha256: Option<String>,
    /// Unix seconds after which the link stops resolving; 0 = never.
    #[serde(default)]
    pub expires_unix: i64,
    #[serde(default)]
    pub disabled: bool,
    /// Denormalized Review-target scope, written at mint (issue #272):
    /// the serving routes run per HTTP hit — including every video
    /// byte-range request — and must not re-scan the vault to learn
    /// which file a review pins.
    #[serde(default)]
    pub review_root_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub review_file_path: Option<String>,
    pub created_at: String,
}

impl StoredLink {
    /// The link's target in canonical form: a pre-`target` row is a
    /// note of the org's own vault, and a note row's empty `vault_id`
    /// (minted before wikis were vaults) reads as that same vault —
    /// so `links_for_target` matches by equality across the format
    /// generations.
    pub fn target(&self) -> ShareTarget {
        self.target
            .clone()
            .unwrap_or_else(|| ShareTarget::note(String::new(), self.note_path.clone()))
            .canonical()
    }

    pub fn capabilities(&self) -> ShareCapabilities {
        self.capabilities.unwrap_or(ShareCapabilities {
            comment: self.capability == "comment",
            download: false,
            file_request: false,
        })
    }

    pub fn expired(&self, now_unix: i64) -> bool {
        self.expires_unix > 0 && now_unix > self.expires_unix
    }

    /// Check a presented password. An unprotected link accepts anything;
    /// a protected one requires the exact hash match.
    pub fn password_ok(&self, presented: Option<&str>) -> bool {
        match &self.password_sha256 {
            None => true,
            Some(hash) => presented.is_some_and(|pw| sha256_hex(pw) == *hash),
        }
    }
}

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let out = h.finalize();
    let mut hex = String::with_capacity(64);
    for b in out {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// On-disk shape of `shares.json`. Loads the pre-#271 shape (a bare
/// array of links) transparently.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct StoreFile {
    #[serde(default)]
    links: Vec<StoredLink>,
    /// The org kill switch: while on, no new link mints.
    #[serde(default)]
    sharing_disabled: bool,
}

/// JSON-file-backed link store, one per org, plus the append-only
/// access log beside it.
pub struct ShareStore {
    path: PathBuf,
    log_path: PathBuf,
    /// The org dir — file-request uploads land under
    /// `<org>/files-incoming/<token>/` (issue #272 AC 3).
    org_dir: PathBuf,
    state: Mutex<StoreFile>,
}

impl ShareStore {
    /// Open (or start empty at) `<org>/shares.json`.
    pub fn open(org_dir: &std::path::Path) -> Self {
        let path = org_dir.join("shares.json");
        let log_path = org_dir.join("shares-access.jsonl");
        let state = match std::fs::read_to_string(&path) {
            Err(_) => StoreFile::default(),
            Ok(body) => serde_json::from_str::<StoreFile>(&body)
                .ok()
                .or_else(|| {
                    // Pre-#271 shape: a bare array of links.
                    serde_json::from_str::<Vec<StoredLink>>(&body)
                        .ok()
                        .map(|links| StoreFile {
                            links,
                            sharing_disabled: false,
                        })
                })
                .unwrap_or_else(|| {
                    // An unreadable store must NOT silently become an
                    // empty one — the next save would destroy every
                    // link and fail the kill switch open. Preserve the
                    // bytes aside, loudly, and start fresh.
                    let backup = path.with_extension(format!(
                        "json.unreadable-{}",
                        chrono::Utc::now().timestamp()
                    ));
                    let moved = std::fs::rename(&path, &backup);
                    tracing::error!(
                        path = %path.display(),
                        backup = %backup.display(),
                        moved = moved.is_ok(),
                        "shares.json is unreadable — preserved aside; links must be re-minted"
                    );
                    StoreFile::default()
                }),
        };
        Self {
            path,
            log_path,
            org_dir: org_dir.to_path_buf(),
            state: Mutex::new(state),
        }
    }

    /// A file-request link's incoming area (per token, created lazily).
    pub fn incoming_dir(&self, token: &str) -> PathBuf {
        self.org_dir.join("files-incoming").join(token)
    }

    fn save_locked(path: &std::path::Path, file: &StoreFile) -> Result<(), ShareError> {
        let json =
            serde_json::to_string_pretty(file).map_err(|e| ShareError::Storage(e.to_string()))?;
        std::fs::write(path, json).map_err(|e| ShareError::Storage(e.to_string()))
    }

    pub fn resolve(&self, token: &str) -> Option<StoredLink> {
        self.state
            .lock()
            .expect("share store poisoned")
            .links
            .iter()
            .find(|l| l.token == token)
            .cloned()
    }

    pub fn sharing_disabled(&self) -> bool {
        self.state
            .lock()
            .expect("share store poisoned")
            .sharing_disabled
    }

    /// Mutate and persist under ONE lock hold: two concurrent mutations
    /// must not race their `fs::write`s, or the older snapshot can land
    /// last and silently undo a revocation across restart.
    fn mutate<R>(&self, f: impl FnOnce(&mut StoreFile) -> R) -> Result<R, ShareError> {
        let mut guard = self.state.lock().expect("share store poisoned");
        let r = f(&mut guard);
        Self::save_locked(&self.path, &guard)?;
        Ok(r)
    }

    /// Read-only view (no save).
    fn with_state<R>(&self, f: impl FnOnce(&StoreFile) -> R) -> R {
        let guard = self.state.lock().expect("share store poisoned");
        f(&guard)
    }

    /// Append one access row — landing views, browses, rendition
    /// streams, and download receipts (issue #271 AC 4). Best-effort:
    /// a failed log line must not fail the serve.
    pub fn log_access(&self, token: &str, kind: &str, path: &str) {
        #[derive(serde::Serialize)]
        struct Row<'a> {
            at: String,
            token: &'a str,
            kind: &'a str,
            path: &'a str,
        }
        let row = Row {
            at: chrono::Utc::now().to_rfc3339(),
            token,
            kind,
            path,
        };
        let Ok(line) = serde_json::to_string(&row) else {
            return;
        };
        use std::io::Write as _;
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .and_then(|mut f| writeln!(f, "{line}"));
    }

    /// One link's access rows, newest first (capped — the log is
    /// append-only and unbounded on disk).
    pub fn read_access(&self, token: &str) -> Vec<ShareAccess> {
        #[derive(serde::Deserialize)]
        struct Row {
            at: String,
            token: String,
            kind: String,
            #[serde(default)]
            path: String,
        }
        // Tail-bounded: the log grows without limit (anonymous token
        // holders append on every hit), so read at most the last 512 KB
        // and drop the first (possibly partial) line.
        const TAIL: u64 = 512 * 1024;
        let body = (|| -> std::io::Result<String> {
            use std::io::{Read as _, Seek as _, SeekFrom};
            let mut f = std::fs::File::open(&self.log_path)?;
            let len = f.metadata()?.len();
            let start = len.saturating_sub(TAIL);
            f.seek(SeekFrom::Start(start))?;
            let mut s = String::new();
            f.read_to_string(&mut s)?;
            if start > 0 {
                if let Some(nl) = s.find('\n') {
                    s.drain(..=nl);
                }
            }
            Ok(s)
        })();
        let Ok(body) = body else {
            return Vec::new();
        };
        let mut out: Vec<ShareAccess> = body
            .lines()
            .filter_map(|l| serde_json::from_str::<Row>(l).ok())
            .filter(|r| r.token == token)
            .map(|r| ShareAccess {
                at: r.at,
                kind: r.kind,
                path: r.path,
            })
            .collect();
        out.reverse();
        out.truncate(500);
        out
    }
}

// ── the org-lane service ────────────────────────────────────────────

/// The org-lane share service.
#[derive(Clone)]
pub struct ShareServiceImpl {
    store: std::sync::Arc<ShareStore>,
    slug: String,
    /// Public base URL links are composed against
    /// (`TASK_SHARE_PUBLIC_BASE`, default derived from the bind address).
    public_base: String,
    /// The org's Files backend — Slice mints validate the root's flavor
    /// against it. `None` only in contexts with no Files mounted.
    files: Option<files::FilesBackend>,
}

impl ShareServiceImpl {
    pub fn new(
        store: std::sync::Arc<ShareStore>,
        slug: String,
        public_base: String,
        files: Option<files::FilesBackend>,
    ) -> Self {
        Self {
            store,
            slug,
            public_base,
            files,
        }
    }

    fn info(&self, l: &StoredLink) -> ShareLinkInfo {
        ShareLinkInfo {
            token: l.token.clone(),
            label: l.label.clone(),
            target: l.target(),
            capabilities: l.capabilities(),
            password_protected: l.password_sha256.is_some(),
            expires_unix: l.expires_unix,
            disabled: l.disabled,
            url: format!(
                "{}/org/{}/share/{}",
                self.public_base.trim_end_matches('/'),
                self.slug,
                l.token
            ),
            created_at: l.created_at.clone(),
        }
    }
}

fn validate_target(target: &ShareTarget) -> Result<(), ShareError> {
    match target {
        ShareTarget::Note { path, .. } if path.is_empty() => {
            Err(ShareError::Invalid("empty note path".into()))
        }
        // A note lives in the org's own vault or in one of its wikis;
        // any other vault id is a client bug, not a link to mint.
        ShareTarget::Note { vault_id, .. }
            if vault_id != share_proto::DEFAULT_VAULT
                && crate::wiki_vault::wiki_of(vault_id).is_none() =>
        {
            Err(ShareError::Invalid(format!("bad note vault: {vault_id}")))
        }
        ShareTarget::Slice { subpath, .. }
            if subpath.starts_with('/') || subpath.split('/').any(|s| s == "..") =>
        {
            Err(ShareError::Invalid(format!("bad slice subpath: {subpath}")))
        }
        _ => Ok(()),
    }
}

/// Apply mint/edit options onto a link. `fresh` distinguishes the
/// update contract (None keeps, sentinel clears) from a fresh mint.
fn apply_options(
    link: &mut StoredLink,
    options: NewShareLink,
    fresh: bool,
) -> Result<(), ShareError> {
    if options.expires_unix.is_some_and(|t| t < 0) {
        return Err(ShareError::Invalid(
            "expires_unix must be a unix timestamp (or 0 to clear)".into(),
        ));
    }
    if !options.label.is_empty() || fresh {
        link.label = if options.label.is_empty() {
            "share link".into()
        } else {
            options.label
        };
    }
    // `None` keeps on update — a partial edit ("just set a password")
    // must not silently rewrite the download grant.
    match options.capabilities {
        Some(caps) => link.capabilities = Some(caps),
        None if fresh => link.capabilities = Some(ShareCapabilities::default()),
        None => {}
    }
    match options.password.as_deref() {
        Some("") => link.password_sha256 = None,
        Some(pw) => link.password_sha256 = Some(sha256_hex(pw)),
        None if fresh => link.password_sha256 = None,
        None => {}
    }
    match options.expires_unix {
        Some(0) => link.expires_unix = 0,
        Some(t) => link.expires_unix = t,
        None if fresh => link.expires_unix = 0,
        None => {}
    }
    Ok(())
}

impl ShareService for ShareServiceImpl {
    async fn create_link(
        &self,
        target: ShareTarget,
        options: NewShareLink,
    ) -> Result<ShareLinkInfo, ShareError> {
        // The org kill switch (issue #271): minting refuses; existing
        // links keep resolving until individually revoked.
        if self.store.sharing_disabled() {
            return Err(ShareError::SharingDisabled);
        }
        // Stored in canonical form (a note names its vault), so the
        // row compares equal to what the panel later asks for.
        let target = target.canonical();
        validate_target(&target)?;
        // A Slice link's byte-serving paths (rendition + download) only
        // exist on Media roots — minting one for a software root would
        // hand out a link where every file 404s. Refuse at the mint.
        if let ShareTarget::Slice { root_id, .. } = &target {
            let Some(files) = &self.files else {
                return Err(ShareError::Invalid(
                    "slice links need the Files backend mounted".into(),
                ));
            };
            let root = files
                .get_root(*root_id)
                .await
                .map_err(|e| ShareError::Invalid(format!("root: {e}")))?;
            if root.flavor != files::RootFlavor::Media {
                return Err(ShareError::Invalid(
                    "only Media roots can be shared as slices (software roots come later)".into(),
                ));
            }
        }
        // A Review link must point at a review that exists — a guest
        // lane on a dangling id would 404 on every open. The review's
        // scope is denormalized onto the link so serving never re-scans
        // the vault per hit.
        let mut review_scope: Option<(uuid::Uuid, String)> = None;
        if let ShareTarget::Review { id } = &target {
            let Some(files) = &self.files else {
                return Err(ShareError::Invalid(
                    "review links need the Files backend mounted".into(),
                ));
            };
            let review = files
                .get_review(*id)
                .await
                .map_err(|e| ShareError::Invalid(format!("review: {e}")))?;
            review_scope = Some((review.root_id, review.file_path));
        }
        // 32 hex chars of UUIDv4 randomness — unguessable, URL-safe.
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut link = StoredLink {
            token,
            label: String::new(),
            target: Some(target),
            note_path: String::new(),
            capability: String::new(),
            capabilities: None,
            password_sha256: None,
            expires_unix: 0,
            disabled: false,
            review_root_id: review_scope.as_ref().map(|(root, _)| *root),
            review_file_path: review_scope.map(|(_, path)| path),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        apply_options(&mut link, options, true)?;
        self.store.mutate(|s| {
            s.links.insert(0, link.clone());
            self.info(&link)
        })
    }

    async fn update_link(
        &self,
        token: String,
        options: NewShareLink,
    ) -> Result<ShareLinkInfo, ShareError> {
        let info = self.store.mutate(|s| {
            s.links
                .iter_mut()
                .find(|l| l.token == token)
                .map(|l| apply_options(l, options, false).map(|()| self.info(l)))
        })?;
        match info {
            Some(result) => result,
            None => Err(ShareError::NotFound),
        }
    }

    async fn list_links(&self) -> Result<Vec<ShareLinkInfo>, ShareError> {
        Ok(self
            .store
            .with_state(|s| s.links.iter().map(|l| self.info(l)).collect()))
    }

    async fn links_for_target(
        &self,
        target: ShareTarget,
    ) -> Result<Vec<ShareLinkInfo>, ShareError> {
        let target = target.canonical();
        Ok(self.store.with_state(|s| {
            s.links
                .iter()
                .filter(|l| l.target() == target)
                .map(|l| self.info(l))
                .collect()
        }))
    }

    async fn set_link_disabled(&self, token: String, disabled: bool) -> Result<(), ShareError> {
        let found = self
            .store
            .mutate(|s| match s.links.iter_mut().find(|l| l.token == token) {
                Some(l) => {
                    l.disabled = disabled;
                    true
                }
                None => false,
            })?;
        if !found {
            return Err(ShareError::NotFound);
        }
        Ok(())
    }

    async fn delete_link(&self, token: String) -> Result<(), ShareError> {
        let found = self.store.mutate(|s| {
            let before = s.links.len();
            s.links.retain(|l| l.token != token);
            s.links.len() != before
        })?;
        if !found {
            return Err(ShareError::NotFound);
        }
        Ok(())
    }

    async fn access_log(&self, token: String) -> Result<Vec<ShareAccess>, ShareError> {
        if self.store.resolve(&token).is_none() {
            return Err(ShareError::NotFound);
        }
        // The log is an unbounded append-only file that anonymous token
        // holders grow — read its tail off the async worker.
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.read_access(&token))
            .await
            .map_err(|e| ShareError::Storage(format!("log read: {e}")))
    }

    async fn set_sharing_disabled(&self, disabled: bool) -> Result<(), ShareError> {
        self.store.mutate(|s| s.sharing_disabled = disabled)
    }

    async fn sharing_disabled(&self) -> Result<bool, ShareError> {
        Ok(self.store.sharing_disabled())
    }

    async fn list_incoming(
        &self,
        token: String,
    ) -> Result<Vec<share_proto::IncomingFile>, ShareError> {
        if self.store.resolve(&token).is_none() {
            return Err(ShareError::NotFound);
        }
        let dir = self.store.incoming_dir(&token);
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let Ok(meta) = entry.metadata() else { continue };
                if !meta.is_file() {
                    continue;
                }
                let uploaded_at = meta
                    .modified()
                    .ok()
                    .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
                    .unwrap_or_default();
                out.push(share_proto::IncomingFile {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    size: meta.len(),
                    uploaded_at,
                });
            }
        }
        out.sort_by(|a, b| b.uploaded_at.cmp(&a.uploaded_at));
        Ok(out)
    }

    async fn promote_incoming(
        &self,
        token: String,
        name: String,
        dest_path: String,
    ) -> Result<(), ShareError> {
        let Some(link) = self.store.resolve(&token) else {
            return Err(ShareError::NotFound);
        };
        // Only a Slice link has a tree to promote into.
        let ShareTarget::Slice { root_id, subpath } = link.target() else {
            return Err(ShareError::Invalid("not a slice link".into()));
        };
        // The upload's name is a flat basename (the upload route wrote
        // it); refuse anything that isn't.
        if name.is_empty() || name.contains('/') || name.contains("..") {
            return Err(ShareError::Invalid(format!("bad incoming name: {name}")));
        }
        let dest_path = dest_path.trim_matches('/');
        if dest_path.is_empty() || dest_path.split('/').any(|s| s == ".." || s.is_empty()) {
            return Err(ShareError::Invalid(format!("bad destination: {dest_path}")));
        }
        // The destination is SLICE-relative: a link scoped to
        // `client-deliverables/` promotes into that subtree only —
        // the same visibility boundary its browsing had.
        let dest_rel = if subpath.is_empty() {
            dest_path.to_string()
        } else {
            format!("{subpath}/{dest_path}")
        };
        let Some(files) = &self.files else {
            return Err(ShareError::Invalid(
                "promotion needs the Files backend mounted".into(),
            ));
        };
        let src = self.store.incoming_dir(&token).join(&name);
        if !src.is_file() {
            return Err(ShareError::NotFound);
        }
        // `place_file` carries the live-tree write safeguards the raw
        // copy lacked: confinement (symlinked parents included), the
        // root lock (a cadence capture can't snapshot a half-copied
        // file), and never-overwrite.
        files
            .place_file(root_id, dest_rel.clone(), src.clone())
            .await
            .map_err(|e| ShareError::Invalid(format!("promote: {e}")))?;
        let _ = std::fs::remove_file(&src);
        // The cadence engine sees the arrival like any other save; a
        // hint makes the capture prompt.
        let _ = files.hint_activity(root_id, vec![dest_rel]).await;
        Ok(())
    }
}

// ── HTTP: token-gated serving ───────────────────────────────────────
//
// These routes are the link's whole surface. They are token-gated, not
// session-gated, on purpose: the token IS the grant, checked fresh on
// every hit, and the scope is enforced by construction — a slice
// link's URLs carry paths relative to the slice, so nothing outside it
// is addressable, signed in or not (AC 1).

/// Query string every share route accepts: the link password.
#[derive(serde::Deserialize, Default)]
pub struct ShareQuery {
    #[serde(default)]
    pub pw: Option<String>,
}

/// The gate every share hit passes: resolve, disabled/expired → 410,
/// password → form/401. `Ok` carries the link. `interactive` marks
/// HTML pages, where a MISSING password renders the form as 200; byte
/// routes (download / rendition) must instead refuse with 401 — a curl
/// or a `<video>` would otherwise save the form HTML as the file and
/// call it success. (The refusal Response is boxed — clippy's
/// `result_large_err`; every caller immediately unboxes.)
fn gate(
    state: &AppState,
    slug: &str,
    token: &str,
    pw: Option<&str>,
    interactive: bool,
) -> Result<(crate::OrgAppState, StoredLink), Box<Response>> {
    // Wide-event enrichment (see the logging skill): the span is the
    // wide event; every share hit records what it resolved to. Shape
    // only — the token itself never lands in a field.
    use architect_telemetry::wide;
    wide::set("org.slug", slug.to_owned());
    let Some(org) = state.org(slug) else {
        wide::set("share.outcome", "org-not-found");
        return Err(Box::new(
            (StatusCode::NOT_FOUND, "no such org").into_response(),
        ));
    };
    let Some(link) = org.shares.resolve(token) else {
        wide::set("share.outcome", "link-not-found");
        return Err(Box::new(
            (StatusCode::NOT_FOUND, "no such share link").into_response(),
        ));
    };
    wide::set("share.label", link.label.clone());
    wide::set(
        "share.target_kind",
        match link.target() {
            ShareTarget::Note { .. } => "note",
            ShareTarget::Slice { .. } => "slice",
            ShareTarget::NamedVersion { .. } => "named-version",
            ShareTarget::Review { .. } => "review",
        },
    );
    if link.disabled {
        wide::set("share.outcome", "disabled");
        return Err(Box::new(
            (StatusCode::GONE, Html(disabled_html())).into_response(),
        ));
    }
    if link.expired(chrono::Utc::now().timestamp()) {
        wide::set("share.outcome", "expired");
        return Err(Box::new(
            (StatusCode::GONE, Html(expired_html())).into_response(),
        ));
    }
    if !link.password_ok(pw) {
        wide::set(
            "share.outcome",
            if pw.is_some() {
                "password-wrong"
            } else {
                "password-missing"
            },
        );
        return Err(Box::new(match (pw, interactive) {
            // Wrong password ≠ no password: the former is a refusal,
            // the latter is the form (on HTML pages only).
            (Some(_), _) => (
                StatusCode::UNAUTHORIZED,
                Html(password_html(&link.label, true)),
            )
                .into_response(),
            (None, true) => {
                (StatusCode::OK, Html(password_html(&link.label, false))).into_response()
            }
            (None, false) => (
                StatusCode::UNAUTHORIZED,
                "this share link is password-protected — pass ?pw=",
            )
                .into_response(),
        }));
    }
    wide::set("share.outcome", "ok");
    Ok((org, link))
}

/// Reject a slice-relative path that tries to escape.
fn clean_rel(rel: &str) -> Result<String, Box<Response>> {
    let rel = rel.trim_matches('/');
    if rel.split('/').any(|s| s == "..") {
        return Err(Box::new(StatusCode::NOT_FOUND.into_response()));
    }
    Ok(rel.to_string())
}

/// Join the slice subpath with a link-relative path.
fn join_scope(subpath: &str, rel: &str) -> String {
    match (subpath.is_empty(), rel.is_empty()) {
        (true, _) => rel.to_string(),
        (false, true) => subpath.to_string(),
        (false, false) => format!("{subpath}/{rel}"),
    }
}

/// What a Files share resolves to: the root and (for a Named Version)
/// the exact commit to serve.
struct FilesScope {
    root_id: uuid::Uuid,
    subpath: String,
    /// `None` = the checkpoint head, re-resolved per request (a slice
    /// link follows the root as it moves; a Named Version link pins).
    /// Browse LISTINGS resolve the head too ([`render_browse`]), so a
    /// link never lists a file its byte routes can't serve.
    at: Option<String>,
    /// A Review link (issue #272) is scoped to ONE file: only this
    /// root-relative path is addressable; browsing is refused.
    file_only: Option<String>,
}

/// Resolve a link's Files scope; `None` for note links.
async fn files_scope(
    org: &crate::OrgAppState,
    link: &StoredLink,
) -> Result<Option<FilesScope>, Response> {
    match link.target() {
        ShareTarget::Note { .. } => Ok(None),
        ShareTarget::Slice { root_id, subpath } => Ok(Some(FilesScope {
            root_id,
            subpath,
            at: None,
            file_only: None,
        })),
        ShareTarget::NamedVersion { id } => {
            // Resolve the curated entity to its exact change — the
            // whole point of a Named Version link (AC 2). The
            // `VersionRef` carries the root too; no org-wide scan.
            let version = org.files.resolve_named_version(id).await.map_err(|e| {
                (StatusCode::NOT_FOUND, format!("named version: {e}")).into_response()
            })?;
            Ok(Some(FilesScope {
                root_id: version.root_id,
                subpath: String::new(),
                at: Some(version.commit_id),
                file_only: None,
            }))
        }
        ShareTarget::Review { id } => {
            // The review pins the file; the guest lane (its own vox
            // route) carries the conversation. These HTML routes serve
            // exactly the one file. The scope is denormalized on the
            // link at mint — this runs on every byte-range request and
            // must not scan the vault; the targeted lookup is only the
            // legacy fallback for links minted before the field.
            let (root_id, file_path) = match (&link.review_root_id, &link.review_file_path) {
                (Some(root), Some(path)) => (*root, path.clone()),
                _ => {
                    let review = org.files.get_review(id).await.map_err(|e| {
                        (StatusCode::NOT_FOUND, format!("review: {e}")).into_response()
                    })?;
                    (review.root_id, review.file_path)
                }
            };
            Ok(Some(FilesScope {
                root_id,
                subpath: String::new(),
                at: None,
                file_only: Some(file_path),
            }))
        }
    }
}

fn pw_suffix(q: &ShareQuery) -> String {
    match &q.pw {
        Some(pw) if !pw.is_empty() => format!("?pw={}", urlencoding_encode(pw)),
        _ => String::new(),
    }
}

/// `GET /org/{slug}/share/{token}` — the landing: note links keep their
/// open-in-app card; Files links render the scoped listing directly.
pub async fn share_landing_handler(
    State(state): State<AppState>,
    AxPath((slug, token)): AxPath<(String, String)>,
    Query(q): Query<ShareQuery>,
) -> Response {
    let (org, link) = match gate(&state, &slug, &token, q.pw.as_deref(), true) {
        Ok(v) => v,
        Err(resp) => return *resp,
    };
    org.shares.log_access(&token, "view", "");
    match files_scope(&org, &link).await {
        Err(resp) => resp,
        Ok(None) => {
            let app_origin = std::env::var("TASK_SHARE_APP_ORIGIN").unwrap_or_default();
            Html(landing_html(&slug, &link, &app_origin)).into_response()
        }
        Ok(Some(scope)) => render_browse(&org, &slug, &token, &link, &scope, "", &q).await,
    }
}

/// `GET /org/{slug}/share/{token}/b/{*rel}` — browse inside the scope.
pub async fn share_browse_handler(
    State(state): State<AppState>,
    AxPath((slug, token, rel)): AxPath<(String, String, String)>,
    Query(q): Query<ShareQuery>,
) -> Response {
    let (org, link) = match gate(&state, &slug, &token, q.pw.as_deref(), true) {
        Ok(v) => v,
        Err(resp) => return *resp,
    };
    let rel = match clean_rel(&rel) {
        Ok(r) => r,
        Err(resp) => return *resp,
    };
    let scope = match files_scope(&org, &link).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, "not a files link").into_response(),
        Err(resp) => return resp,
    };
    org.shares.log_access(&token, "browse", &rel);
    render_browse(&org, &slug, &token, &link, &scope, &rel, &q).await
}

/// `GET /org/{slug}/share/{token}/rendition/{kind}/{*rel}` — stream a
/// derived rendition. This is the ONLY media a view-only link serves:
/// originals need the `download` capability (AC 3).
pub async fn share_rendition_handler(
    State(state): State<AppState>,
    AxPath((slug, token, kind, rel)): AxPath<(String, String, String, String)>,
    Query(q): Query<ShareQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    let (org, link) = match gate(&state, &slug, &token, q.pw.as_deref(), false) {
        Ok(v) => v,
        Err(resp) => return *resp,
    };
    let rel = match clean_rel(&rel) {
        Ok(r) => r,
        Err(resp) => return *resp,
    };
    let scope = match files_scope(&org, &link).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, "not a files link").into_response(),
        Err(resp) => return resp,
    };
    let Some(wire_kind) = rendition_kind_from_tag(&kind) else {
        return (StatusCode::NOT_FOUND, "unknown rendition kind").into_response();
    };
    if let Some(only) = &scope.file_only
        && rel != *only
    {
        return (StatusCode::NOT_FOUND, "this link serves one file").into_response();
    }
    let full = join_scope(&scope.subpath, &rel);
    let rendition = match &scope.at {
        Some(commit) => {
            org.files
                .rendition_at(scope.root_id, full, commit.clone(), wire_kind)
                .await
        }
        None => org.files.rendition(scope.root_id, full, wire_kind).await,
    };
    let info = match rendition {
        Ok(info) => info,
        Err(e) => return (StatusCode::NOT_FOUND, format!("rendition: {e}")).into_response(),
    };
    org.shares.log_access(&token, "rendition", &rel);
    let total = info.len;
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| crate::parse_byte_range(s, total));
    crate::rendition_stream_response(&org, scope.root_id, &info.file_id, &info.mime, total, range)
}

/// `GET /org/{slug}/share/{token}/download/{*rel}` — original bytes,
/// gated by the `download` capability, receipted in the access log.
pub async fn share_download_handler(
    State(state): State<AppState>,
    AxPath((slug, token, rel)): AxPath<(String, String, String)>,
    Query(q): Query<ShareQuery>,
) -> Response {
    let (org, link) = match gate(&state, &slug, &token, q.pw.as_deref(), false) {
        Ok(v) => v,
        Err(resp) => return *resp,
    };
    if !link.capabilities().download {
        return (
            StatusCode::FORBIDDEN,
            "this link is view-only — downloads are not enabled",
        )
            .into_response();
    }
    let rel = match clean_rel(&rel) {
        Ok(r) => r,
        Err(resp) => return *resp,
    };
    let scope = match files_scope(&org, &link).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, "not a files link").into_response(),
        Err(resp) => return resp,
    };
    if let Some(only) = &scope.file_only
        && rel != *only
    {
        return (StatusCode::NOT_FOUND, "this link serves one file").into_response();
    }
    let full = join_scope(&scope.subpath, &rel);
    // Pin the exact bytes BEFORE any header leaves: `resolve_source`
    // returns (len, CAS id) in one resolution and the stream then reads
    // by that immutable id — a checkpoint landing mid-request can't
    // produce a Content-Length/body mismatch. A slice link (at = None)
    // pins the current head; a Named Version link pins its commit.
    let (len, content_id) = match org
        .files
        .resolve_source(scope.root_id, full.clone(), scope.at.clone())
        .await
    {
        Ok(v) => v,
        Err(e) => return (StatusCode::NOT_FOUND, format!("download: {e}")).into_response(),
    };
    // The receipt (AC 4) — written before the stream starts, so an
    // aborted transfer still shows who pulled the trigger.
    org.shares.log_access(&token, "download", &rel);
    let filename = sanitize_filename(rel.rsplit('/').next().unwrap_or("download"));
    let (mut writer, reader) = tokio::io::duplex(64 * 1024);
    let files = org.files.clone();
    let root_id = scope.root_id;
    tokio::spawn(async move {
        if let Err(e) = files
            .read_source_content(root_id, &content_id, &mut writer)
            .await
        {
            tracing::warn!(?e, "share download: read failed mid-stream");
        }
    });
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(reader));
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_LENGTH, len.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response()
}

/// A `Content-Disposition`-safe filename: control bytes would make the
/// header value unbuildable (a bare 500 after the receipt logged), and
/// `"`/`\` break the quoted-string. Conservative allowlist; empty
/// results fall back to "download".
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || " ._-()[]".contains(*c))
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "download".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `GET /org/{slug}/share/{token}/vox` — the guest lane (issue #272):
/// an anonymous WebSocket carrying the real RPC surface, scoped to the
/// link's Review by construction (see [`crate::share_guest`]). The
/// token + password + expiry are the whole grant, re-checked at every
/// upgrade; there is no session and no permission-gate principal.
pub async fn share_guest_vox_handler(
    State(state): State<AppState>,
    AxPath((slug, token)): AxPath<(String, String)>,
    Query(q): Query<ShareQuery>,
    ws: axum::extract::WebSocketUpgrade,
) -> Response {
    let (org, link) = match gate(&state, &slug, &token, q.pw.as_deref(), false) {
        Ok(v) => v,
        Err(resp) => return *resp,
    };
    let ShareTarget::Review { id } = link.target() else {
        return (StatusCode::BAD_REQUEST, "not a review link").into_response();
    };
    let review = match org.files.get_review(id).await {
        Ok(review) => review,
        Err(e) => return (StatusCode::NOT_FOUND, format!("review: {e}")).into_response(),
    };
    org.shares.log_access(&token, "guest", "");
    let guest = crate::share_guest::GuestFilesService::new(
        org.files.clone(),
        review.clone(),
        org.shares.clone(),
        &link,
    );
    let media = crate::share_guest::GuestMediaService::new(
        crate::media::AttachmentMediaServiceImpl::new(
            org.attachments.clone(),
            org.slug.clone(),
            org.attachments.keypair.clone(),
        ),
        org.shares.clone(),
        &link,
        review.root_id,
    );
    // The stream sibling rides too — the review page's live comment
    // subscription works over the guest lane, fed by the service's own
    // filtered event mirror (never the org-wide hub).
    let router = architect::LayerRouter::new()
        .merge(files::files_service_layer(guest.clone()))
        .merge(files_proto::files_service_stream_layer(guest))
        .with(
            media_proto::attachment_media_descriptor(),
            media_proto::AttachmentMediaDispatcher::new(media),
        );
    let router = crate::snapshot::GatedRouter::new(router, state.write_gate.clone());
    ws.protocols([crate::VOX_SUBPROTOCOL])
        .on_upgrade(move |socket| architect::axum_ws::serve_router(socket, router))
}

/// `POST /org/{slug}/share/{token}/upload/{name}` — the file-request
/// inbox (issue #272 AC 3): visitors with the `file_request` capability
/// drop files into the link's per-token incoming area. Uploads NEVER
/// touch the tree (no overwrite, no delete — collisions get suffixed);
/// the owner promotes them in.
pub async fn share_upload_handler(
    State(state): State<AppState>,
    AxPath((slug, token, name)): AxPath<(String, String, String)>,
    Query(q): Query<ShareQuery>,
    request: axum::extract::Request,
) -> Response {
    /// A token's whole incoming area is bounded — anonymous holders
    /// must not be able to fill the disk.
    const MAX_INCOMING_FILES: usize = 100;
    const MAX_INCOMING_BYTES: u64 = 8 * 1024 * 1024 * 1024;

    let (org, link) = match gate(&state, &slug, &token, q.pw.as_deref(), false) {
        Ok(v) => v,
        Err(resp) => return *resp,
    };
    if !link.capabilities().file_request {
        return (
            StatusCode::FORBIDDEN,
            "this link does not accept file uploads",
        )
            .into_response();
    }
    // Only a Slice link has a tree to promote into — an upload against
    // any other target would be stranded forever.
    if !matches!(link.target(), ShareTarget::Slice { .. }) {
        return (
            StatusCode::FORBIDDEN,
            "file requests only work on slice links",
        )
            .into_response();
    }
    // Flat basenames only — the incoming area is a single review queue,
    // not a tree.
    let name = sanitize_filename(name.rsplit('/').next().unwrap_or(&name));
    let dir = org.shares.incoming_dir(&token);
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("incoming: {e}")).into_response();
    }
    // Quota check (best-effort snapshot; the body-limit layer bounds
    // any single request).
    let (mut count, mut bytes) = (0usize, 0u64);
    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            if let Ok(meta) = entry.metadata().await {
                count += 1;
                bytes += meta.len();
            }
        }
    }
    if count >= MAX_INCOMING_FILES || bytes >= MAX_INCOMING_BYTES {
        return (
            StatusCode::INSUFFICIENT_STORAGE,
            "this link's incoming area is full — ask the owner to review it",
        )
            .into_response();
    }
    // Claim a destination with `create_new` — atomic against both the
    // suffix loop falling through and two concurrent same-name uploads
    // (the old exists()→write pair was TOCTOU-racy and could clobber).
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    let mut file_and_name = None;
    for n in 1..=1000u32 {
        let candidate_name = if n == 1 {
            name.clone()
        } else {
            format!("{stem}-{n}{ext}")
        };
        match tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(dir.join(&candidate_name))
            .await
        {
            Ok(file) => {
                file_and_name = Some((file, candidate_name));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("upload: {e}")).into_response();
            }
        }
    }
    let Some((mut file, saved)) = file_and_name else {
        // 1000 collisions = someone is scripting; the quota above would
        // have said "full" first in any honest flow.
        return (StatusCode::INSUFFICIENT_STORAGE, "too many name collisions").into_response();
    };
    // Stream the body to disk — never buffer an anonymous upload in
    // RAM (the body-limit layer caps the request; the heap must not
    // pay for it).
    let stream = request.into_body().into_data_stream();
    let mut reader = tokio_util::io::StreamReader::new(stream.map_err(std::io::Error::other));
    match tokio::io::copy(&mut reader, &mut file).await {
        Ok(_) => {}
        Err(e) => {
            let _ = tokio::fs::remove_file(dir.join(&saved)).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("upload: {e}")).into_response();
        }
    }
    org.shares.log_access(&token, "upload", &saved);
    (StatusCode::OK, saved).into_response()
}

fn rendition_kind_from_tag(tag: &str) -> Option<files_proto::RenditionKind> {
    use files_proto::RenditionKind as K;
    Some(match tag {
        "proxy-1080" => K::Proxy1080,
        "proxy-720" => K::Proxy720,
        "audio-aac" => K::Audio,
        "peaks" => K::Peaks,
        "filmstrip" => K::Filmstrip,
        _ => return None,
    })
}

/// Render a directory listing (or bounce to the file page) at `rel`
/// inside the scope.
async fn render_browse(
    org: &crate::OrgAppState,
    slug: &str,
    token: &str,
    link: &StoredLink,
    scope: &FilesScope,
    rel: &str,
    q: &ShareQuery,
) -> Response {
    // A Review link is one file, not a tree: the landing (rel = "")
    // IS the file page, any other path 404s, and there is no listing.
    // The page carries "Open review" — the real app booting in guest
    // mode over the scoped vox lane (issue #272), where commenting and
    // drawing live.
    if let Some(only) = &scope.file_only {
        let base = format!("/org/{slug}/share/{token}");
        let pw = pw_suffix(q);
        return if rel.is_empty() || rel == *only {
            let open_review = review_app_url(slug, token, link, q);
            Html(file_html(link, &base, only, &pw, open_review.as_deref())).into_response()
        } else {
            (StatusCode::NOT_FOUND, "this link serves one file").into_response()
        };
    }
    let full = join_scope(&scope.subpath, rel);
    // List the CHECKPOINT tree, never the live directory: the byte
    // routes (download / rendition) serve checkpointed content, and a
    // listing must not show a file they would 404 on (or serve stale
    // bytes for). A slice link resolves the head fresh per request.
    let commit = match &scope.at {
        Some(commit) => commit.clone(),
        None => match org.files.head_commit_hex(scope.root_id).await {
            Ok(head) => head,
            Err(e) => {
                return (StatusCode::NOT_FOUND, format!("root: {e}")).into_response();
            }
        },
    };
    let listing = org
        .files
        .browse_at(scope.root_id, commit, full.clone())
        .await;
    let base = format!("/org/{slug}/share/{token}");
    let pw = pw_suffix(q);
    match listing {
        Ok(entries) => Html(browse_html(link, &base, rel, &entries, &pw)).into_response(),
        // Not a directory (or not present as one): render the file page.
        Err(_) => Html(file_html(link, &base, rel, &pw, None)).into_response(),
    }
}

// ── HTML ────────────────────────────────────────────────────────────

const PAGE_CSS: &str = r"
 body{font-family:system-ui,sans-serif;background:#0b0d10;color:#e6e8eb;margin:0;padding:2rem;display:flex;justify-content:center}
 .card{background:#14171c;border:1px solid #262b33;border-radius:12px;padding:2rem;max-width:44rem;width:100%}
 h1{font-size:1.1rem;margin:0 0 .3rem}
 p{color:#9aa3af;font-size:.9rem}
 a{color:#a5b4fc;text-decoration:none}
 a:hover{text-decoration:underline}
 ul{list-style:none;padding:0;margin:.8rem 0}
 li{padding:.45rem .2rem;border-bottom:1px solid #1d222a;display:flex;gap:.6rem;align-items:baseline}
 .cap{display:inline-block;border:1px solid #333a45;border-radius:99px;padding:.1rem .6rem;font-size:.72rem;color:#9aa3af;margin:0 .25rem .8rem 0;text-transform:uppercase;letter-spacing:.08em}
 .crumb{font-size:.85rem;color:#9aa3af;margin-bottom:.6rem}
 video{width:100%;border-radius:8px;background:#000}
 .btn{display:inline-block;background:#6d5ef2;color:#fff;border-radius:8px;padding:.5rem 1.1rem;font-weight:600;margin-top:1rem}
 input{background:#0b0d10;border:1px solid #333a45;border-radius:8px;color:#e6e8eb;padding:.5rem .8rem}
";

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn page(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>{title}</title><style>{PAGE_CSS}</style></head>
<body><div class="card">{body}</div></body></html>"#,
        title = html_escape(title),
    )
}

fn caps_chips(link: &StoredLink) -> String {
    let caps = link.capabilities();
    let mut out = String::from(r#"<span class="cap">view</span>"#);
    if caps.comment {
        out.push_str(r#"<span class="cap">comment</span>"#);
    }
    if caps.download {
        out.push_str(r#"<span class="cap">download</span>"#);
    }
    out
}

fn browse_html(
    link: &StoredLink,
    base: &str,
    rel: &str,
    entries: &[files_proto::BrowseEntry],
    pw: &str,
) -> String {
    let label = html_escape(&link.label);
    let mut body = format!("<h1>{label}</h1>{}", caps_chips(link));
    if !rel.is_empty() {
        let parent = match rel.rsplit_once('/') {
            Some((p, _)) => format!("{base}/b/{p}{pw}"),
            None => format!("{base}{pw}"),
        };
        body.push_str(&format!(
            r#"<div class="crumb"><a href="{parent}">&larr; up</a> &nbsp; /{rel}</div>"#,
            rel = html_escape(rel),
        ));
    }
    body.push_str("<ul>");
    for e in entries {
        let child = if rel.is_empty() {
            e.name.clone()
        } else {
            format!("{rel}/{}", e.name)
        };
        let icon = if e.is_dir { "&#128193;" } else { "&#128196;" };
        body.push_str(&format!(
            r#"<li>{icon} <a href="{base}/b/{child}{pw}">{name}</a></li>"#,
            child = urlencoding_encode(&child),
            name = html_escape(&e.name),
        ));
    }
    if entries.is_empty() {
        body.push_str("<li>empty</li>");
    }
    body.push_str("</ul>");
    page(&link.label, &body)
}

/// Video extensions the share file page embeds a proxy player for —
/// mirror of the app's review-player list.
fn is_video_name(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or_default().to_lowercase();
    matches!(
        ext.as_str(),
        "mov" | "mp4" | "m4v" | "mkv" | "webm" | "avi" | "mxf" | "mts"
    )
}

/// The guest app's review URL for a landing's "Open review" button —
/// `None` when the button shouldn't render: the link doesn't grant
/// commenting (view-only stays view-only end to end), or no app is
/// reachable (neither `TASK_SHARE_APP_ORIGIN` nor a served SPA), where
/// the href would 404.
fn review_app_url(slug: &str, token: &str, link: &StoredLink, q: &ShareQuery) -> Option<String> {
    if !link.capabilities().comment {
        return None;
    }
    let app_origin = std::env::var("TASK_SHARE_APP_ORIGIN").unwrap_or_default();
    let serves_spa = std::env::var("TASK_SERVER_WEB_DIR").is_ok_and(|v| !v.is_empty());
    if app_origin.is_empty() && !serves_spa {
        return None;
    }
    let pw_param = match &q.pw {
        Some(pw) if !pw.is_empty() => format!("&pw={}", urlencoding_encode(pw)),
        _ => String::new(),
    };
    // `server` tells the guest app where its vox lane lives — a fresh
    // visitor has no server registry, and same-origin fallback only
    // holds when app and server share an origin (prod). Dev splits
    // them (dx vs task-server). Only an EXPLICIT public base rides
    // along: the bind-address fallback would tell a remote browser to
    // dial 127.0.0.1.
    let server_param = match crate::share_public_base_explicit() {
        Some(base) => format!("&server={}", urlencoding_encode(&base)),
        None => String::new(),
    };
    Some(format!(
        "{}/share-review?org={slug}&token={token}{pw_param}{server_param}",
        app_origin.trim_end_matches('/'),
    ))
}

fn file_html(
    link: &StoredLink,
    base: &str,
    rel: &str,
    pw: &str,
    open_review: Option<&str>,
) -> String {
    let label = html_escape(&link.label);
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let mut body = format!(
        "<h1>{label}</h1>{}<div class=\"crumb\">/{rel}</div>",
        caps_chips(link),
        rel = html_escape(rel),
    );
    if is_video_name(name) {
        body.push_str(&format!(
            r#"<video controls preload="metadata" src="{base}/rendition/proxy-720/{rel}{pw}"></video>"#,
            rel = urlencoding_encode(rel),
        ));
    } else {
        body.push_str(&format!("<p>{}</p>", html_escape(name)));
    }
    if link.capabilities().download {
        body.push_str(&format!(
            r#"<a class="btn" href="{base}/download/{rel}{pw}">Download original</a>"#,
            rel = urlencoding_encode(rel),
        ));
    } else {
        body.push_str(r#"<p>View-only link — originals aren't downloadable.</p>"#);
    }
    if let Some(url) = open_review {
        // The review landing's door into the real app on the guest
        // lane (issue #272).
        body.push_str(&format!(
            r#"<a class="btn" style="margin-left:.6rem" href="{}">Open review — comment &amp; draw</a>"#,
            html_escape(url),
        ));
    }
    page(&link.label, &body)
}

fn password_html(label: &str, wrong: bool) -> String {
    let note = if wrong {
        r#"<p style="color:#f87171">Wrong password.</p>"#
    } else {
        "<p>This link is password-protected.</p>"
    };
    let body = format!(
        r#"<h1>{label}</h1>{note}
<form method="get"><input type="password" name="pw" placeholder="Password" autofocus>
<button class="btn" type="submit">Open</button></form>"#,
        label = html_escape(label),
    );
    page(label, &body)
}

/// The share landing page for a NOTE link: token-checked on EVERY hit
/// (revocation is immediate), then a minimal page that opens the shared
/// note in the app. `app_origin` = where the web app lives
/// (`TASK_SHARE_APP_ORIGIN`; empty = same origin).
pub fn landing_html(slug: &str, link: &StoredLink, app_origin: &str) -> String {
    let (note, vault_id) = match link.target() {
        ShareTarget::Note { path, vault_id } => (path, vault_id),
        _ => (String::new(), String::new()),
    };
    let label = html_escape(&link.label);
    let open = format!(
        "{}{}&share=1",
        app_origin.trim_end_matches('/'),
        note_open_path(slug, &vault_id, &note)
    );
    let body = format!(
        r#"<h1>{label}</h1>{caps}
<p>You've been invited to <strong>{note}</strong>.</p>
<a class="btn" href="{open}">Open</a>"#,
        caps = caps_chips(link),
        note = html_escape(&note),
    );
    page(&link.label, &body)
}

/// The web app path that opens a shared note, by the vault it lives
/// in: the org's own vault opens on the vault route, a wiki's page on
/// that wiki's page route (the client's `routes::note_route`, mirrored
/// — a share link to a wiki page must land in the wiki, not the vault).
/// Query-shaped so the caller can append `&share=1`.
pub fn note_open_path(slug: &str, vault_id: &str, note: &str) -> String {
    match crate::wiki_vault::wiki_of(vault_id) {
        Some(wiki) => format!(
            "/wiki/w/{}/{}/page?path={}",
            urlencoding_encode(slug),
            urlencoding_encode(wiki),
            urlencoding_encode(note)
        ),
        None => format!("/vault?path={}", urlencoding_encode(note)),
    }
}

/// Gone page for a disabled link.
pub fn disabled_html() -> &'static str {
    r#"<!doctype html><html><head><meta charset="utf-8"><title>Link disabled</title>
<style>body{font-family:system-ui,sans-serif;background:#0b0d10;color:#9aa3af;display:flex;min-height:100vh;align-items:center;justify-content:center}</style>
</head><body><p>This share link has been disabled by its owner.</p></body></html>"#
}

/// Gone page for an expired link.
pub fn expired_html() -> &'static str {
    r#"<!doctype html><html><head><meta charset="utf-8"><title>Link expired</title>
<style>body{font-family:system-ui,sans-serif;background:#0b0d10;color:#9aa3af;display:flex;min-height:100vh;align-items:center;justify-content:center}</style>
</head><body><p>This share link has expired.</p></body></html>"#
}

/// Percent-encode a path for a URL (kept tiny to avoid a dep).
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod note_target_tests {
    use super::*;

    fn stored(target: Option<ShareTarget>, note_path: &str) -> StoredLink {
        StoredLink {
            token: "t".into(),
            label: "l".into(),
            target,
            note_path: note_path.into(),
            capability: String::new(),
            capabilities: None,
            password_sha256: None,
            expires_unix: 0,
            disabled: false,
            review_root_id: None,
            review_file_path: None,
            created_at: String::new(),
        }
    }

    /// Every generation of stored note link reads as the same canonical
    /// target, so a panel query matches rows minted before wikis were
    /// vaults.
    #[test]
    fn stored_note_links_canonicalise_to_the_default_vault() {
        let legacy_row = stored(None, "Plans.md");
        let empty_vault = stored(
            Some(ShareTarget::Note {
                path: "Plans.md".into(),
                vault_id: String::new(),
            }),
            "",
        );
        let today = stored(
            Some(ShareTarget::note(share_proto::DEFAULT_VAULT, "Plans.md")),
            "",
        );
        let want = ShareTarget::note("default", "Plans.md");
        assert_eq!(legacy_row.target(), want);
        assert_eq!(empty_vault.target(), want);
        assert_eq!(today.target(), want);
        // A wiki page keeps its vault — it is a different note.
        let wiki = stored(Some(ShareTarget::note("wiki:music-theory", "Plans.md")), "");
        assert_ne!(wiki.target(), want);
    }

    #[test]
    fn note_targets_validate_their_vault() {
        assert!(validate_target(&ShareTarget::note("default", "A.md")).is_ok());
        assert!(validate_target(&ShareTarget::note("wiki:music-theory", "A.md")).is_ok());
        assert!(validate_target(&ShareTarget::note("wiki:music-theory", "")).is_err());
        assert!(validate_target(&ShareTarget::note("shares", "A.md")).is_err());
    }

    /// The landing page opens a wiki page on the wiki route and a vault
    /// note on the vault route — the client's `note_route`, mirrored.
    #[test]
    fn landing_opens_the_note_where_it_lives() {
        assert_eq!(
            note_open_path("acme", "default", "Plans.md"),
            "/vault?path=Plans.md"
        );
        assert_eq!(
            note_open_path("acme", "wiki:music-theory", "Concepts/Modes.md"),
            "/wiki/w/acme/music-theory/page?path=Concepts/Modes.md"
        );
        let link = stored(
            Some(ShareTarget::note("wiki:music-theory", "Concepts/Modes.md")),
            "",
        );
        let html = landing_html("acme", &link, "https://app.example");
        assert!(
            html.contains(
                "https://app.example/wiki/w/acme/music-theory/page?path=Concepts/Modes.md&share=1"
            ),
            "{html}"
        );
    }
}
