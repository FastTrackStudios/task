//! Who outside the owning org may read an asset shelf or a project.
//!
//! A wiki has always said this for itself, in `_state/wiki.json`
//! (`wiki.access.visibility`). Asset shelves and projects carried nothing,
//! and `LocalOrgs::admits` took that to mean *everyone*: any org on the
//! server that could name a shelf or a project could subscribe to it. That
//! was recorded as a decision rather than discovered, but it was the wrong
//! one. Both are now **private by default** (`wiki.access.shelf-visibility`)
//! and open to an outsider in exactly two ways:
//!
//! - **Visibility.** `<shelf>/.shelf.toml` says `visibility = "unlisted"`
//!   or `"public"`, the same three words a wiki uses and with the same
//!   meaning. No file, or a file that will not parse, is private. A dot
//!   file, so the vault walker never syncs the publisher's own setting
//!   into a subscriber's copy.
//! - **A grant.** An owner or admin of the publishing org grants one
//!   source to one subscriber org (`wiki.access.source-grant`). Grants are
//!   rows in an append-only log: a revoke is a new line naming the grant,
//!   never an edit or a delete, so who could read what, and when, stays
//!   on record.
//!
//! Both are read on every admission, so narrowing a shelf or revoking a
//! grant binds on the subscriber's next refresh. The copy already held is
//! kept: what ends is the refreshing (`wiki.life.orphan`).
//!
//! # Projects nest, so a declaration covers what sits under it
//!
//! A project's slug is a path (`crescendum/track-two`), and subscribing to
//! a parent copies its sub-projects with it. So a sub-project with no
//! declaration of its own takes its nearest ancestor's, and a grant on a
//! parent admits its sub-projects too. Admitting the child by reference
//! when the parent's copy already carries it would be a refusal that
//! protects nothing.
//!
//! # The remote path
//!
//! A subscriber on another server has no org on this disk, so it cannot
//! be named by slug. A grant therefore carries a secret, returned once
//! when it is minted and stored only as a SHA-256. The publisher's
//! `source_manifest` / `source_file` resolve a presented secret to its
//! grant, and judge the caller as the subscriber that grant names. One
//! revoke ends both paths.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use wiki_proto::config::Visibility;
use wiki_proto::subscription::SourceKind;

/// The file a shelf or a project declares its visibility in.
pub const SHELF_CONFIG: &str = ".shelf.toml";

/// The log of grants, under the publisher's own bookkeeping directory,
/// beside `granted.json` (`crate::source_grants`).
const GRANTS_LOG: &str = "access-grants.jsonl";

/// Whether this kind of source is governed here. A wiki has its own
/// config and a Resource is public by licence (`wiki.resource.rights`);
/// neither is a shelf for this purpose.
#[must_use]
pub const fn governs(kind: SourceKind) -> bool {
    matches!(kind, SourceKind::Assets | SourceKind::Projects)
}

/// What `.shelf.toml` holds.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct ShelfConfig {
    #[serde(default)]
    visibility: Visibility,
}

/// Where `root`'s declaration lives.
#[must_use]
pub fn config_path(root: &Path) -> PathBuf {
    root.join(SHELF_CONFIG)
}

/// What `root` declares, if it declares anything readable.
///
/// A file that will not parse counts as no declaration, and so as
/// private. A typo should not open a shelf.
fn declared(root: &Path) -> Option<Visibility> {
    let text = std::fs::read_to_string(config_path(root)).ok()?;
    match toml::from_str::<ShelfConfig>(&text) {
        Ok(c) => Some(c.visibility),
        Err(e) => {
            tracing::warn!(
                path = %config_path(root).display(),
                error = %e,
                "unreadable shelf visibility; treating the shelf as private"
            );
            Some(Visibility::Private)
        }
    }
}

/// The visibility `root` is held at.
///
/// `tier_root` is the tier directory (`<org>/assets`, `<org>/projects`).
/// For a project, the nearest declaration from `root` up to, but not
/// including, `tier_root` wins. For a shelf, only `root` itself counts.
/// With no declaration anywhere, private.
#[must_use]
pub fn visibility(tier_root: &Path, root: &Path) -> Visibility {
    let mut at = Some(root);
    while let Some(dir) = at {
        if dir == tier_root || !dir.starts_with(tier_root) {
            break;
        }
        if let Some(v) = declared(dir) {
            return v;
        }
        at = dir.parent();
    }
    Visibility::Private
}

/// Declare `root`'s visibility.
///
/// # Errors
///
/// A failure writing the file.
pub fn set_visibility(root: &Path, visibility: Visibility) -> std::io::Result<()> {
    let text = toml::to_string(&ShelfConfig { visibility }).map_err(std::io::Error::other)?;
    let body = format!(
        "# Who outside this org may subscribe: public, unlisted or private.\n\
         # Without this file the shelf is private.\n{text}"
    );
    std::fs::write(config_path(root), body)
}

/// One grant: one source of this org, to one subscriber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessGrant {
    pub id: uuid::Uuid,
    pub kind: SourceKind,
    /// The source's slug. For a project, its tier-relative path.
    pub slug: String,
    /// The subscriber: an org slug on this server, or the federation
    /// domain of an org on another one.
    pub subscriber: String,
    /// Who granted it, as the principal that called.
    pub granted_by: String,
    /// Unix seconds.
    pub granted_at: i64,
    pub revoked_at: Option<i64>,
    pub revoked_by: Option<String>,
    /// SHA-256 of the secret a remote subscriber presents. Never the
    /// secret itself.
    secret_sha256: String,
}

impl AccessGrant {
    /// In force: not revoked.
    #[must_use]
    pub const fn is_live(&self) -> bool {
        self.revoked_at.is_none()
    }

    /// Whether this grant reaches `slug` of `kind`: the same source, or,
    /// for a project, a sub-project of the granted one.
    fn reaches(&self, kind: SourceKind, slug: &str) -> bool {
        if self.kind != kind {
            return false;
        }
        if self.slug == slug {
            return true;
        }
        kind == SourceKind::Projects
            && slug
                .strip_prefix(self.slug.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

/// One line of the log.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Line {
    Granted {
        id: uuid::Uuid,
        kind: SourceKind,
        slug: String,
        subscriber: String,
        secret_sha256: String,
        by: String,
        at: i64,
    },
    Revoked {
        id: uuid::Uuid,
        by: String,
        at: i64,
    },
    /// A visibility change, kept in the same log so the record of who
    /// opened what is in one place.
    Visibility {
        kind: SourceKind,
        slug: String,
        visibility: Visibility,
        by: String,
        at: i64,
    },
}

/// A grant just minted, with the secret it can be read by. The secret is
/// in this value and nowhere else. It is not stored and cannot be
/// recovered.
#[derive(Debug, Clone)]
pub struct Minted {
    pub grant: AccessGrant,
    pub secret: String,
}

/// What [`AccessGrants::revoke`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revoked {
    pub grant: AccessGrant,
    /// `true` only for the call that actually revoked it.
    pub newly: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AccessError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
    /// A live grant for exactly this source and subscriber already
    /// exists. Revoke it first: a grant is never edited.
    #[error("`{subscriber}` already holds a live grant on this source ({id})")]
    AlreadyGranted { id: uuid::Uuid, subscriber: String },
}

/// An org's grant log.
#[derive(Debug, Clone)]
pub struct AccessGrants {
    path: PathBuf,
}

fn sha256_hex(s: &str) -> String {
    let digest = Sha256::digest(s.as_bytes());
    digest.iter().fold(String::with_capacity(64), |mut out, b| {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// A subscriber name: an org slug, or a domain. Lowercase ASCII letters,
/// digits, `-` and `.`, not starting or ending with a separator.
fn valid_subscriber(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && !s.starts_with(['-', '.'])
        && !s.ends_with(['-', '.'])
        && !s.contains("..")
}

impl AccessGrants {
    /// Open the log for an org root. Nothing is read until asked.
    #[must_use]
    pub fn open(org_root: &Path) -> Self {
        Self {
            path: org_root.join("subscribed").join(".state").join(GRANTS_LOG),
        }
    }

    /// Every grant, oldest first, with its revocation folded in.
    ///
    /// `Err` when any line will not parse. Callers that *admit* treat that
    /// as no grants at all: a damaged line could be a revoke, and skipping
    /// it would bring a revoked grant back.
    ///
    /// # Errors
    ///
    /// A read failure other than "no log yet", or an unparseable line.
    pub fn list(&self) -> Result<Vec<AccessGrant>, AccessError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out: Vec<AccessGrant> = Vec::new();
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let parsed: Line = serde_json::from_str(line).map_err(|e| {
                AccessError::Invalid(format!("{}:{}: {e}", self.path.display(), n + 1))
            })?;
            match parsed {
                Line::Granted {
                    id,
                    kind,
                    slug,
                    subscriber,
                    secret_sha256,
                    by,
                    at,
                } => out.push(AccessGrant {
                    id,
                    kind,
                    slug,
                    subscriber,
                    granted_by: by,
                    granted_at: at,
                    revoked_at: None,
                    revoked_by: None,
                    secret_sha256,
                }),
                Line::Revoked { id, by, at } => {
                    // The first revoke is the one on record.
                    if let Some(g) = out.iter_mut().find(|g| g.id == id && g.is_live()) {
                        g.revoked_at = Some(at);
                        g.revoked_by = Some(by);
                    }
                }
                Line::Visibility { .. } => {}
            }
        }
        Ok(out)
    }

    /// Live grants, or none when the log cannot be read.
    fn live(&self) -> Vec<AccessGrant> {
        match self.list() {
            Ok(all) => all.into_iter().filter(AccessGrant::is_live).collect(),
            Err(e) => {
                tracing::warn!(
                    path = %self.path.display(),
                    error = %e,
                    "unreadable access-grant log; admitting nobody by grant"
                );
                Vec::new()
            }
        }
    }

    fn append(&self, line: &Line) -> Result<(), AccessError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut bytes = serde_json::to_vec(line).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        // One write of one line, so concurrent appenders do not interleave.
        f.write_all(&bytes)?;
        f.sync_data()?;
        Ok(())
    }

    /// Grant `slug` of `kind` to `subscriber`.
    ///
    /// Does not check that the source exists. The caller does that, as
    /// the owner, before granting.
    ///
    /// # Errors
    ///
    /// An ungovernable kind, a bad subscriber name, a live grant already
    /// held, or a failure writing the log.
    pub fn grant(
        &self,
        kind: SourceKind,
        slug: &str,
        subscriber: &str,
        by: &str,
        now: i64,
    ) -> Result<Minted, AccessError> {
        if !governs(kind) {
            return Err(AccessError::Invalid(format!(
                "a grant is for an asset shelf or a project, not a {}: a wiki's \
                 visibility is its own config",
                kind.noun()
            )));
        }
        if !valid_subscriber(subscriber) {
            return Err(AccessError::Invalid(
                "the subscriber is an org slug or a federation domain".into(),
            ));
        }
        if let Some(existing) = self
            .list()?
            .into_iter()
            .find(|g| g.is_live() && g.kind == kind && g.slug == slug && g.subscriber == subscriber)
        {
            return Err(AccessError::AlreadyGranted {
                id: existing.id,
                subscriber: existing.subscriber,
            });
        }
        let secret = uuid::Uuid::new_v4().to_string();
        let grant = AccessGrant {
            id: uuid::Uuid::new_v4(),
            kind,
            slug: slug.to_owned(),
            subscriber: subscriber.to_owned(),
            granted_by: by.to_owned(),
            granted_at: now,
            revoked_at: None,
            revoked_by: None,
            secret_sha256: sha256_hex(&secret),
        };
        self.append(&Line::Granted {
            id: grant.id,
            kind,
            slug: grant.slug.clone(),
            subscriber: grant.subscriber.clone(),
            secret_sha256: grant.secret_sha256.clone(),
            by: by.to_owned(),
            at: now,
        })?;
        Ok(Minted { grant, secret })
    }

    /// Revoke grant `id`. `None` when there is no such grant; a second
    /// revoke of the same grant writes nothing and reports `newly: false`.
    ///
    /// # Errors
    ///
    /// A failure reading or writing the log.
    pub fn revoke(
        &self,
        id: uuid::Uuid,
        by: &str,
        now: i64,
    ) -> Result<Option<Revoked>, AccessError> {
        let Some(mut grant) = self.list()?.into_iter().find(|g| g.id == id) else {
            return Ok(None);
        };
        if !grant.is_live() {
            return Ok(Some(Revoked {
                grant,
                newly: false,
            }));
        }
        self.append(&Line::Revoked {
            id,
            by: by.to_owned(),
            at: now,
        })?;
        grant.revoked_at = Some(now);
        grant.revoked_by = Some(by.to_owned());
        Ok(Some(Revoked { grant, newly: true }))
    }

    /// Record a visibility change in the log. The setting itself is the
    /// file; this is the audit copy of who made it.
    ///
    /// # Errors
    ///
    /// A failure writing the log.
    pub fn record_visibility(
        &self,
        kind: SourceKind,
        slug: &str,
        visibility: Visibility,
        by: &str,
        now: i64,
    ) -> Result<(), AccessError> {
        self.append(&Line::Visibility {
            kind,
            slug: slug.to_owned(),
            visibility,
            by: by.to_owned(),
            at: now,
        })
    }

    /// Whether a live grant admits `subscriber` to `slug` of `kind`.
    #[must_use]
    pub fn admits(&self, kind: SourceKind, slug: &str, subscriber: &str) -> bool {
        if subscriber.is_empty() {
            return false;
        }
        self.live()
            .iter()
            .any(|g| g.subscriber == subscriber && g.reaches(kind, slug))
    }

    /// The subscriber a presented `secret` speaks for on `slug` of `kind`,
    /// when it belongs to a live grant that reaches that source.
    #[must_use]
    pub fn subscriber_by_secret(
        &self,
        kind: SourceKind,
        slug: &str,
        secret: &str,
    ) -> Option<String> {
        if secret.is_empty() {
            return None;
        }
        let hashed = sha256_hex(secret);
        self.live()
            .into_iter()
            .find(|g| g.secret_sha256 == hashed && g.reaches(kind, slug))
            .map(|g| g.subscriber)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, AccessGrants) {
        let dir = tempfile::tempdir().unwrap();
        let g = AccessGrants::open(dir.path());
        (dir, g)
    }

    #[test]
    fn no_file_is_private_and_a_declaration_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let tier = dir.path().join("assets");
        let shelf = tier.join("songs");
        std::fs::create_dir_all(&shelf).unwrap();
        assert_eq!(visibility(&tier, &shelf), Visibility::Private);
        set_visibility(&shelf, Visibility::Unlisted).unwrap();
        assert_eq!(visibility(&tier, &shelf), Visibility::Unlisted);
        std::fs::write(config_path(&shelf), "visibility = \"wide open\"\n").unwrap();
        assert_eq!(
            visibility(&tier, &shelf),
            Visibility::Private,
            "a declaration that does not parse is private"
        );
    }

    #[test]
    fn a_sub_project_takes_its_nearest_declaration() {
        let dir = tempfile::tempdir().unwrap();
        let tier = dir.path().join("projects");
        let album = tier.join("crescendum");
        let song = album.join("track-two");
        std::fs::create_dir_all(&song).unwrap();
        assert_eq!(visibility(&tier, &song), Visibility::Private);
        set_visibility(&album, Visibility::Unlisted).unwrap();
        assert_eq!(visibility(&tier, &song), Visibility::Unlisted);
        set_visibility(&song, Visibility::Public).unwrap();
        assert_eq!(visibility(&tier, &song), Visibility::Public);
        // Nothing above the tier counts.
        set_visibility(dir.path(), Visibility::Public).unwrap();
        std::fs::remove_file(config_path(&album)).unwrap();
        std::fs::remove_file(config_path(&song)).unwrap();
        assert_eq!(visibility(&tier, &song), Visibility::Private);
    }

    #[test]
    fn a_grant_admits_its_subscriber_on_its_source_only_until_revoked() {
        let (_d, g) = store();
        let minted = g
            .grant(SourceKind::Assets, "songs", "vnt-video", "owner", 1)
            .unwrap();
        assert!(g.admits(SourceKind::Assets, "songs", "vnt-video"));
        assert!(!g.admits(SourceKind::Assets, "songs", "alice-personal"));
        assert!(!g.admits(SourceKind::Assets, "charts", "vnt-video"));
        assert!(!g.admits(SourceKind::Projects, "songs", "vnt-video"));
        assert!(!g.admits(SourceKind::Assets, "songs", ""));
        assert_eq!(
            g.subscriber_by_secret(SourceKind::Assets, "songs", &minted.secret),
            Some("vnt-video".to_owned())
        );
        assert_eq!(
            g.subscriber_by_secret(SourceKind::Assets, "charts", &minted.secret),
            None
        );

        let r = g.revoke(minted.grant.id, "owner", 2).unwrap().unwrap();
        assert!(r.newly);
        assert!(!g.admits(SourceKind::Assets, "songs", "vnt-video"));
        assert_eq!(
            g.subscriber_by_secret(SourceKind::Assets, "songs", &minted.secret),
            None
        );
        let again = g.revoke(minted.grant.id, "owner", 3).unwrap().unwrap();
        assert!(!again.newly);
        assert_eq!(again.grant.revoked_at, Some(2), "the first revoke stands");
        assert!(
            g.revoke(uuid::Uuid::new_v4(), "owner", 4)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_log_is_append_only_and_never_holds_the_secret() {
        let (dir, g) = store();
        let minted = g
            .grant(SourceKind::Projects, "crescendum", "vnt-video", "owner", 1)
            .unwrap();
        g.revoke(minted.grant.id, "owner", 2).unwrap();
        let again = g
            .grant(SourceKind::Projects, "crescendum", "vnt-video", "owner", 3)
            .unwrap();
        let text =
            std::fs::read_to_string(dir.path().join("subscribed/.state/access-grants.jsonl"))
                .unwrap();
        assert_eq!(text.lines().count(), 3, "grant, revoke, grant: {text}");
        assert!(!text.contains(&minted.secret) && !text.contains(&again.secret));
        let all = g.list().unwrap();
        assert_eq!(all.len(), 2);
        assert!(!all[0].is_live() && all[1].is_live());
    }

    #[test]
    fn a_grant_on_a_project_reaches_its_sub_projects_and_nothing_beside_it() {
        let (_d, g) = store();
        g.grant(SourceKind::Projects, "crescendum", "vnt-video", "o", 1)
            .unwrap();
        assert!(g.admits(SourceKind::Projects, "crescendum/track-two", "vnt-video"));
        assert!(!g.admits(SourceKind::Projects, "crescendum-deluxe", "vnt-video"));
    }

    #[test]
    fn a_live_duplicate_is_refused_and_bad_input_is_refused() {
        let (_d, g) = store();
        g.grant(SourceKind::Assets, "songs", "vnt-video", "o", 1)
            .unwrap();
        assert!(matches!(
            g.grant(SourceKind::Assets, "songs", "vnt-video", "o", 2),
            Err(AccessError::AlreadyGranted { .. })
        ));
        assert!(matches!(
            g.grant(SourceKind::Wiki, "theory", "vnt-video", "o", 2),
            Err(AccessError::Invalid(_))
        ));
        for bad in ["", "VNT", "a/b", "..", "-x", "x."] {
            assert!(
                matches!(
                    g.grant(SourceKind::Assets, "songs", bad, "o", 2),
                    Err(AccessError::Invalid(_))
                ),
                "{bad:?}"
            );
        }
        assert!(
            g.grant(SourceKind::Assets, "songs", "vnt.test", "o", 2)
                .is_ok()
        );
    }

    #[test]
    fn a_damaged_log_admits_nobody() {
        let (dir, g) = store();
        g.grant(SourceKind::Assets, "songs", "vnt-video", "o", 1)
            .unwrap();
        let path = dir.path().join("subscribed/.state/access-grants.jsonl");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"event\":\"revoked\",\"id\":\"not-a-uu\n");
        std::fs::write(&path, text).unwrap();
        assert!(!g.admits(SourceKind::Assets, "songs", "vnt-video"));
        assert!(g.list().is_err());
    }
}
