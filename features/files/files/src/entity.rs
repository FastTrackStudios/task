//! The Vault mapping for Files' two curated version entities (issue
//! #261): [`NamedVersion`] and [`ProjectVersion`].
//!
//! ADR 0001: "Named Versions and Project Versions are Vault entities,
//! not engine constructs ... the version store knows nothing about
//! names." So they live exactly where every other Task entity lives —
//! a markdown page with YAML frontmatter under the org vault,
//! discovered by a live filesystem scan — and everything generic (the
//! frontmatter split, the lenient YAML readers, the slug rule, CRUD)
//! comes from `vault-entity`, the shared slice layer. That is also
//! what makes them replicate: they are vault files, so the existing
//! vault sync carries them offline-first and any device re-resolves
//! them against its own copy of the same root.
//!
//! What stays here is only the field mapping. The frontmatter is built
//! key-by-key rather than by serializing the wire model, because the
//! wire model carries two fields a page must not repeat: `path` (the
//! page's own location — the file system already says it) and `note`
//! (the markdown body).

use chrono::{DateTime, Utc};
use files_proto::{AnnotationStroke, NamedVersion, ProjectVersion, Review, ReviewComment};
use uuid::Uuid;
use vault::VaultPage;
use vault_entity::error::{ParseError, WriteError};
use vault_entity::store::VaultEntity;
use vault_entity::{frontmatter, yaml};

/// Vault folder holding every Files version entity, per root:
/// `Files/<root-slug>/versions/…`,
/// `Files/<root-slug>/project-versions/…`, and
/// `Files/<root-slug>/reviews/…` (issue #270).
pub(crate) const FILES_FOLDER: &str = "Files";
pub(crate) const NAMED_SUBFOLDER: &str = "versions";
pub(crate) const PROJECT_SUBFOLDER: &str = "project-versions";
pub(crate) const REVIEWS_SUBFOLDER: &str = "reviews";

/// Vault mapping marker for [`NamedVersion`].
pub struct NamedVersions;

/// Vault mapping marker for [`ProjectVersion`].
pub struct ProjectVersions;

/// Vault mapping marker for [`Review`] (issue #270).
pub struct Reviews;

/// Vault mapping marker for [`ReviewComment`] (issue #270).
pub struct ReviewComments;

/// Read a uuid from `key`, accepting `alt` as the snake_case spelling
/// a hand-written page might use.
fn uuid_at(map: &serde_yaml::Mapping, key: &str, alt: &str) -> Option<Uuid> {
    yaml::str_at(map, key)
        .or_else(|| yaml::str_at(map, alt))
        .and_then(|s| Uuid::parse_str(&s).ok())
}

fn id_at(map: &serde_yaml::Mapping, rel_path: &str) -> Uuid {
    yaml::str_at(map, "id")
        .and_then(|s| Uuid::parse_str(&s).ok())
        // A page with no `id` still needs a stable identity — derive
        // one from its path, the same way `milestone` does, so a
        // hand-written version page keeps working across reads.
        .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, rel_path.as_bytes()))
}

/// Fallback timestamp for a page with no `dateCreated`. Deliberately
/// the epoch and not `Utc::now()`: parsing happens on *every* read, so
/// "now" would make a page's own timestamp change between two calls
/// and reorder the list under the caller. The epoch is stable, and
/// `on_create` treats it as "unstamped" when the page is next written.
fn epoch() -> DateTime<Utc> {
    DateTime::UNIX_EPOCH
}

fn hex_at(map: &serde_yaml::Mapping, key: &str, alt: &str) -> Option<String> {
    yaml::str_at(map, key)
        .or_else(|| yaml::str_at(map, alt))
        .filter(|s| !s.is_empty())
}

impl VaultEntity for NamedVersions {
    type Model = NamedVersion;

    const TYPE: &'static str = "files-named-version";
    /// Only a fallback: real paths are per-root and built by
    /// [`crate::versions`] from the root's own name.
    const DEFAULT_FOLDER: &'static str = FILES_FOLDER;

    fn id(m: &NamedVersion) -> Uuid {
        m.id
    }
    fn set_id(m: &mut NamedVersion, id: Uuid) {
        m.id = id;
    }
    fn path(m: &NamedVersion) -> &str {
        &m.path
    }
    fn set_path(m: &mut NamedVersion, path: String) {
        m.path = path;
    }
    fn name(m: &NamedVersion) -> &str {
        &m.name
    }

    fn on_create(m: &mut NamedVersion, now: DateTime<Utc>) {
        if m.created_at.timestamp() == 0 {
            m.created_at = now;
        }
    }

    fn from_page(page: &VaultPage) -> Result<NamedVersion, ParseError> {
        let (map, body) = frontmatter::mapping(&page.raw).ok_or(ParseError::NoFrontmatter)?;
        let root_id = uuid_at(&map, "rootId", "root_id").ok_or_else(|| {
            ParseError::Field("named version is missing required `rootId`".into())
        })?;
        let commit_id = hex_at(&map, "commitId", "commit_id").ok_or_else(|| {
            ParseError::Field("named version is missing required `commitId`".into())
        })?;
        Ok(NamedVersion {
            id: id_at(&map, &page.rel_path),
            path: page.rel_path.clone(),
            name: yaml::str_at(&map, "name").unwrap_or_else(|| page.basename.clone()),
            root_id,
            // A pre-resolution page may carry only the commit id; the
            // change id is recoverable from the store, so accept it.
            change_id: hex_at(&map, "changeId", "change_id").unwrap_or_default(),
            commit_id,
            note: body.trim_start_matches('\n').to_string(),
            created_at: yaml::timestamp_at(&map, "dateCreated").unwrap_or_else(epoch),
        })
    }

    fn to_markdown(m: &NamedVersion) -> Result<String, WriteError> {
        let mut map = serde_yaml::Mapping::new();
        map.insert("id".into(), m.id.to_string().into());
        map.insert("name".into(), m.name.clone().into());
        map.insert("rootId".into(), m.root_id.to_string().into());
        map.insert("changeId".into(), m.change_id.clone().into());
        map.insert("commitId".into(), m.commit_id.clone().into());
        map.insert("dateCreated".into(), m.created_at.to_rfc3339().into());
        frontmatter::document(Self::TYPE, &map, &m.note)
    }
}

impl VaultEntity for ProjectVersions {
    type Model = ProjectVersion;

    const TYPE: &'static str = "files-project-version";
    const DEFAULT_FOLDER: &'static str = FILES_FOLDER;

    fn id(m: &ProjectVersion) -> Uuid {
        m.id
    }
    fn set_id(m: &mut ProjectVersion, id: Uuid) {
        m.id = id;
    }
    fn path(m: &ProjectVersion) -> &str {
        &m.path
    }
    fn set_path(m: &mut ProjectVersion, path: String) {
        m.path = path;
    }
    /// The label alone — a Project Version's real display name is
    /// `v<number>`, which this signature (returning a borrow) can't
    /// build. Nothing depends on that: this feeds only
    /// [`VaultEntity::default_path`], and [`crate::versions`] always
    /// supplies the `v<number>[-label]` path itself.
    fn name(m: &ProjectVersion) -> &str {
        m.label.as_deref().unwrap_or("project version")
    }

    fn on_create(m: &mut ProjectVersion, now: DateTime<Utc>) {
        if m.started_at.timestamp() == 0 {
            m.started_at = now;
        }
    }

    fn from_page(page: &VaultPage) -> Result<ProjectVersion, ParseError> {
        let (map, _body) = frontmatter::mapping(&page.raw).ok_or(ParseError::NoFrontmatter)?;
        let root_id = uuid_at(&map, "rootId", "root_id").ok_or_else(|| {
            ParseError::Field("project version is missing required `rootId`".into())
        })?;
        let number = yaml::i64_at(&map, "number")
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| {
                ParseError::Field("project version is missing required `number`".into())
            })?;
        Ok(ProjectVersion {
            id: id_at(&map, &page.rel_path),
            path: page.rel_path.clone(),
            root_id,
            number,
            label: yaml::str_at(&map, "label").filter(|s| !s.is_empty()),
            change_id: hex_at(&map, "changeId", "change_id").unwrap_or_default(),
            commit_id: hex_at(&map, "commitId", "commit_id").unwrap_or_default(),
            started_at: yaml::timestamp_at(&map, "dateCreated").unwrap_or_else(epoch),
        })
    }

    fn to_markdown(m: &ProjectVersion) -> Result<String, WriteError> {
        let mut map = serde_yaml::Mapping::new();
        map.insert("id".into(), m.id.to_string().into());
        map.insert("rootId".into(), m.root_id.to_string().into());
        map.insert("number".into(), i64::from(m.number).into());
        if let Some(label) = &m.label {
            map.insert("label".into(), label.clone().into());
        }
        map.insert("changeId".into(), m.change_id.clone().into());
        map.insert("commitId".into(), m.commit_id.clone().into());
        map.insert("dateCreated".into(), m.started_at.to_rfc3339().into());
        frontmatter::document(Self::TYPE, &map, "")
    }
}

impl VaultEntity for Reviews {
    type Model = Review;

    const TYPE: &'static str = "files-review";
    const DEFAULT_FOLDER: &'static str = FILES_FOLDER;

    fn id(m: &Review) -> Uuid {
        m.id
    }
    fn set_id(m: &mut Review, id: Uuid) {
        m.id = id;
    }
    fn path(m: &Review) -> &str {
        &m.path
    }
    fn set_path(m: &mut Review, path: String) {
        m.path = path;
    }
    fn name(m: &Review) -> &str {
        &m.title
    }

    fn on_create(m: &mut Review, now: DateTime<Utc>) {
        if m.created_at.timestamp() == 0 {
            m.created_at = now;
        }
    }

    fn from_page(page: &VaultPage) -> Result<Review, ParseError> {
        let (map, body) = frontmatter::mapping(&page.raw).ok_or(ParseError::NoFrontmatter)?;
        let root_id = uuid_at(&map, "rootId", "root_id")
            .ok_or_else(|| ParseError::Field("review is missing required `rootId`".into()))?;
        let file_path = yaml::str_at(&map, "filePath")
            .or_else(|| yaml::str_at(&map, "file_path"))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ParseError::Field("review is missing required `filePath`".into()))?;
        Ok(Review {
            id: id_at(&map, &page.rel_path),
            path: page.rel_path.clone(),
            root_id,
            file_path,
            title: yaml::str_at(&map, "title").unwrap_or_else(|| page.basename.clone()),
            note: body.trim_start_matches('\n').to_string(),
            created_at: yaml::timestamp_at(&map, "dateCreated").unwrap_or_else(epoch),
        })
    }

    fn to_markdown(m: &Review) -> Result<String, WriteError> {
        let mut map = serde_yaml::Mapping::new();
        map.insert("id".into(), m.id.to_string().into());
        map.insert("title".into(), m.title.clone().into());
        map.insert("rootId".into(), m.root_id.to_string().into());
        map.insert("filePath".into(), m.file_path.clone().into());
        map.insert("dateCreated".into(), m.created_at.to_rfc3339().into());
        frontmatter::document(Self::TYPE, &map, &m.note)
    }
}

impl VaultEntity for ReviewComments {
    type Model = ReviewComment;

    const TYPE: &'static str = "files-review-comment";
    const DEFAULT_FOLDER: &'static str = FILES_FOLDER;

    fn id(m: &ReviewComment) -> Uuid {
        m.id
    }
    fn set_id(m: &mut ReviewComment, id: Uuid) {
        m.id = id;
    }
    fn path(m: &ReviewComment) -> &str {
        &m.path
    }
    fn set_path(m: &mut ReviewComment, path: String) {
        m.path = path;
    }
    /// Only feeds [`VaultEntity::default_path`]; real paths are built
    /// by [`crate::versions`] under the review's own folder.
    fn name(m: &ReviewComment) -> &str {
        &m.author
    }

    fn on_create(m: &mut ReviewComment, now: DateTime<Utc>) {
        if m.created_at.timestamp() == 0 {
            m.created_at = now;
        }
    }

    fn from_page(page: &VaultPage) -> Result<ReviewComment, ParseError> {
        let (map, body) = frontmatter::mapping(&page.raw).ok_or(ParseError::NoFrontmatter)?;
        let review_id = uuid_at(&map, "reviewId", "review_id")
            .ok_or_else(|| ParseError::Field("comment is missing required `reviewId`".into()))?;
        let timecode_secs = yaml::f64_at(&map, "timecode").unwrap_or(0.0);
        // The drawing round-trips through serde: strokes are plain
        // data, and hand-editing them is not a supported workflow — an
        // unreadable `annotation` drops to "no drawing" rather than
        // losing the comment text with it.
        let annotation: Vec<AnnotationStroke> = map
            .get(serde_yaml::Value::from("annotation"))
            .cloned()
            .and_then(|v| serde_yaml::from_value(v).ok())
            .unwrap_or_default();
        Ok(ReviewComment {
            id: id_at(&map, &page.rel_path),
            path: page.rel_path.clone(),
            review_id,
            timecode_secs,
            author: yaml::str_at(&map, "author").unwrap_or_default(),
            body: body.trim_start_matches('\n').to_string(),
            commit_id: hex_at(&map, "commitId", "commit_id").unwrap_or_default(),
            annotation,
            via_link: yaml::str_at(&map, "viaLink").unwrap_or_default(),
            created_at: yaml::timestamp_at(&map, "dateCreated").unwrap_or_else(epoch),
        })
    }

    fn to_markdown(m: &ReviewComment) -> Result<String, WriteError> {
        let mut map = serde_yaml::Mapping::new();
        map.insert("id".into(), m.id.to_string().into());
        map.insert("reviewId".into(), m.review_id.to_string().into());
        map.insert("timecode".into(), m.timecode_secs.into());
        if !m.author.is_empty() {
            map.insert("author".into(), m.author.clone().into());
        }
        if !m.via_link.is_empty() {
            map.insert("viaLink".into(), m.via_link.clone().into());
        }
        map.insert("commitId".into(), m.commit_id.clone().into());
        if !m.annotation.is_empty() {
            let strokes = serde_yaml::to_value(&m.annotation)
                .map_err(|e| WriteError::Yaml(format!("annotation: {e}")))?;
            map.insert("annotation".into(), strokes);
        }
        map.insert("dateCreated".into(), m.created_at.to_rfc3339().into());
        frontmatter::document(Self::TYPE, &map, &m.body)
    }
}
