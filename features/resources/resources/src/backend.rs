//! [`ResourcesBackend`] — serves `resources_proto::ResourcesService`
//! over `<org>/resources/`.
//!
//! Reads: the transcript sidecar the watch view cannot fetch directly
//! (the resources tier isn't the vault). Writes: the sermon sync's
//! `upsert_sermon`, which lays a sermon down as files (see
//! [`crate::sermon`]) and then replaces its `sermon-sync` links in the
//! org's typed-link store — one `sermon:<slug>#t:<secs> → verse:<osis>`
//! per scripture reference the captions carry, which is what makes a
//! sermon appear as a backlink in the scripture reader.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use links_proto::{
    Confidence, LinksService as _, NodeKind, NodeRef, Relation, TypedLink, Visibility,
};
use resources_proto::{
    ChartDoc, ChartSummary, ChartUpsert, ContentRef, LightingDoc, LightingSummary, LightingUpsert,
    PatchDoc, PatchSummary, PatchUpsert, ResourcesError, ResourcesService, SampleDoc,
    SampleSummary, SampleUpsert, SermonResource, SermonSummary, SermonUpsert, TranscriptDoc,
};

use crate::scripture_refs::{self, RefHit};
use crate::types::{AnnotationFile, ResourceKind};
use crate::walker::{LoadedResource, walk};
use crate::{ResourceError, chart, lighting, patch, sample, sermon, sidecar, transcript};

/// `provenance.source_ref` on every link the sync mints — so a re-sync
/// replaces only its own links, never a reader's annotations.
pub const SOURCE_REF: &str = "sermon-sync";

/// The subtree sermons live in, under the org-wide resources root.
const SERMONS_DIR: &str = "sermons";

/// The subtree charts live in, under the org-wide resources root
/// (ADR 0003: `resources/charts/<slug>.kf`).
const CHARTS_DIR: &str = "charts";

/// The subtree Signal's patches live in (ADR 0003:
/// `resources/patches/<slug>/`). The directory names in this block are
/// fixed by `node_homes::library_of` on the server as well as by the
/// ADR — they are the subscription slug a cross-org reader names, so
/// renaming one silently stops cross-org resolution.
const PATCHES_DIR: &str = "patches";

/// The subtree Signal's samples live in — manifests only; the audio is
/// in a File Root (see [`crate::sample`]).
const SAMPLES_DIR: &str = "samples";

/// The subtree Ignition's lighting documents live in.
const LIGHTING_DIR: &str = "lighting";

/// The subtree sermons live in inside a named wiki
/// (`<org>/wikis/<wiki>/Resources/Sermons/`).
pub const WIKI_SERMONS_DIR: &str = "Resources/Sermons";

/// Prefix of a `rel_path` that points into the wikis tree rather than
/// the resources tier (`wikis/<wiki>/Resources/Sermons/...`).
const WIKIS_PREFIX: &str = "wikis/";

/// The `.base` laid down next to a wiki's sermon folders — the table
/// the wiki opens the collection through.
pub const SERMONS_BASE: &str = "Sermons.base";

#[derive(Clone, architect::HasDispatcher)]
pub struct ResourcesBackend {
    /// `<org>/resources`.
    root: Arc<PathBuf>,
    /// `<org>/wikis`, when the host has named wikis — sermons synced
    /// with a `wiki` land under `<wikis>/<wiki>/Resources/Sermons/`.
    wikis: Option<Arc<PathBuf>>,
    /// The org's typed-link store, when the host wires one in.
    links: Option<links::Store>,
}

impl ResourcesBackend {
    #[must_use]
    pub fn new(resources_root: impl Into<PathBuf>) -> Self {
        Self {
            root: Arc::new(resources_root.into()),
            wikis: None,
            links: None,
        }
    }

    /// Attach the named-wikis root (`<org>/wikis`), so a sermon can be
    /// hosted by a wiki instead of the org-wide resources tier.
    #[must_use]
    pub fn with_wikis(mut self, wikis_root: impl Into<PathBuf>) -> Self {
        self.wikis = Some(Arc::new(wikis_root.into()));
        self
    }

    /// Attach the typed-link store `upsert_sermon` writes into.
    #[must_use]
    pub fn with_links(mut self, links: links::Store) -> Self {
        self.links = Some(links);
        self
    }

    fn sermons_root(&self) -> PathBuf {
        self.root.join(SERMONS_DIR)
    }

    /// `<org>/resources/charts` — Keyflow's tier, flat by slug.
    fn charts_root(&self) -> PathBuf {
        self.root.join(CHARTS_DIR)
    }

    /// Every chart manifest, slug-sorted (that is [`walk`]'s order).
    fn charts(&self) -> Vec<LoadedResource> {
        walk(self.charts_root())
            .into_iter()
            .filter(|r| r.resource.kind == crate::types::ResourceKind::Chart)
            .collect()
    }

    fn chart_summary(&self, r: &LoadedResource) -> ChartSummary {
        ChartSummary {
            slug: r.resource.slug.clone(),
            title: r.resource.title.clone(),
            key: r.resource.key.clone(),
            notation: r.resource.notation.clone(),
            sections: r.resource.sections.clone(),
            rel_path: self.rel(&r.path),
            updated_at: r.resource.updated_at.clone(),
        }
    }

    // ── The directory-shaped asset lanes ─────────────────────────
    //
    // Patches, samples and lighting differ only in what their
    // frontmatter says, so everything about *where a file goes* is
    // answered once here and the three lanes below supply the fields.
    // A directory per asset (rather than a chart's flat pair) because
    // ADR 0003's `locate()` looks for the directory, and because these
    // kinds grow sidecars.

    /// Every manifest of one asset kind, slug-sorted.
    fn assets(&self, dir: &str, kind: ResourceKind) -> Vec<LoadedResource> {
        walk(self.root.join(dir))
            .into_iter()
            .filter(|r| r.resource.kind == kind)
            .collect()
    }

    /// The slug an upsert lands on, and the manifest path it writes.
    ///
    /// A known slug keeps the file it is already in — including one a
    /// person moved — so re-saving never forks an asset into a second
    /// directory.
    fn asset_slot(
        &self,
        dir: &str,
        kind: ResourceKind,
        manifest: &str,
        slug_in: &str,
        title: &str,
    ) -> Result<(String, PathBuf), ResourcesError> {
        if title.trim().is_empty() {
            return Err(ResourcesError::BadRequest("title is empty".into()));
        }
        let existing = self.assets(dir, kind);
        let taken: Vec<String> = existing.iter().map(|r| r.resource.slug.clone()).collect();
        let slug = crate::asset::slug_for(&taken, slug_in, title);
        // `slugify` strips every separator, so the slug can never climb
        // out of the tier — but an all-punctuation title yields nothing
        // to name a directory with.
        if slug.is_empty() {
            return Err(ResourcesError::BadRequest(format!(
                "title {title:?} has no sluggable characters"
            )));
        }
        let md_path = existing
            .iter()
            .find(|r| r.resource.slug == slug)
            .map_or_else(
                || self.root.join(dir).join(&slug).join(manifest),
                |r| r.path.clone(),
            );
        Ok((slug, md_path))
    }

    /// Write a manifest and its body sidecar into the asset's
    /// directory. The body is stored verbatim — it is the app's
    /// document, and the server never re-renders it.
    fn lay_down(
        md_path: &Path,
        md: &str,
        body_name: &str,
        body: &str,
    ) -> Result<(), ResourcesError> {
        let dir = md_path
            .parent()
            .ok_or_else(|| ResourcesError::Io("manifest has no directory".into()))?;
        std::fs::create_dir_all(dir).map_err(|e| ResourcesError::Io(e.to_string()))?;
        std::fs::write(md_path, md).map_err(|e| ResourcesError::Io(e.to_string()))?;
        std::fs::write(dir.join(body_name), body).map_err(|e| ResourcesError::Io(e.to_string()))
    }

    /// One asset's body sidecar. A missing one reads as empty rather
    /// than an error: the manifest is the asset, the body is beside it.
    fn read_body(path: &Path) -> Result<String, ResourcesError> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(ResourcesError::Io(e.to_string())),
        }
    }

    /// The asset carrying `slug`, or `None`.
    fn asset(&self, dir: &str, kind: ResourceKind, slug: &str) -> Option<LoadedResource> {
        self.assets(dir, kind)
            .into_iter()
            .find(|r| r.resource.slug == slug)
    }

    /// The [`ContentRef`] a manifest carries — where the asset's bytes
    /// are, in a File Root. Never followed here: this lane records the
    /// binding and the Files lane owns what it points at.
    fn content_of(r: &LoadedResource) -> ContentRef {
        ContentRef {
            root_id: r.resource.content_root.clone(),
            path: r.resource.content_path.clone(),
        }
    }

    /// Delete an asset's whole directory. `false` when there was
    /// nothing there.
    ///
    /// Content in a File Root is untouched — this lane never owned it,
    /// and un-declaring a sample is not the same act as destroying the
    /// audio. References from collections are untouched too: a dangling
    /// reference is a legible state (ADR 0003).
    fn delete_asset(
        &self,
        dir: &str,
        kind: ResourceKind,
        slug: &str,
    ) -> Result<bool, ResourcesError> {
        safe_segment(slug, "slug")?;
        let Some(found) = self.asset(dir, kind, slug) else {
            return Ok(false);
        };
        let root = self.root.join(dir);
        let removed = match found.path.parent() {
            // The ordinary shape: the manifest owns its directory, and
            // the directory is what goes.
            Some(parent) if parent != root && parent.starts_with(&root) => {
                std::fs::remove_dir_all(parent)
            }
            // A manifest somebody flattened into the lane root. Take
            // the file and leave the neighbours alone.
            _ => std::fs::remove_file(&found.path),
        };
        match removed {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(ResourcesError::Io(e.to_string())),
        }
    }

    /// Where sermons of `wiki` live; the org-wide tier for `""`.
    fn sermons_root_for(&self, wiki: &str) -> Result<PathBuf, ResourcesError> {
        if wiki.is_empty() {
            return Ok(self.sermons_root());
        }
        safe_segment(wiki, "wiki")?;
        let wikis = self
            .wikis
            .as_ref()
            .ok_or_else(|| ResourcesError::BadRequest("this server has no named wikis".into()))?;
        let dir = wikis.join(wiki);
        if !dir.is_dir() {
            return Err(ResourcesError::NotFound(format!("wiki {wiki}")));
        }
        Ok(dir.join(WIKI_SERMONS_DIR))
    }

    /// Every sermons root that exists: `("", <resources>/sermons)` plus
    /// one per named wiki that has a `Resources/Sermons/`.
    fn roots(&self) -> Vec<(String, PathBuf)> {
        let mut out = vec![(String::new(), self.sermons_root())];
        if let Some(wikis) = &self.wikis {
            let mut named: Vec<(String, PathBuf)> = std::fs::read_dir(wikis.as_path())
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let root = e.path().join(WIKI_SERMONS_DIR);
                    root.is_dir()
                        .then(|| (e.file_name().to_string_lossy().into_owned(), root))
                })
                .collect();
            named.sort();
            out.extend(named);
        }
        out
    }

    /// Every sermon manifest under every sermons root.
    fn sermons(&self) -> Vec<LoadedResource> {
        self.roots()
            .into_iter()
            .flat_map(|(_, root)| walk(root))
            .filter(|r| r.resource.kind == crate::types::ResourceKind::Sermon)
            .collect()
    }

    /// `(wiki, sermons root)` the path lives under.
    fn home_of(&self, path: &Path) -> (String, PathBuf) {
        self.roots()
            .into_iter()
            .find(|(_, root)| path.starts_with(root))
            .unwrap_or_else(|| (String::new(), self.sermons_root()))
    }

    /// Resources-relative (`sermons/...`) or wikis-relative
    /// (`wikis/<wiki>/...`) form of an absolute path.
    fn rel(&self, path: &Path) -> String {
        if let Some(wikis) = &self.wikis {
            if let Ok(rest) = path.strip_prefix(wikis.as_path()) {
                return format!(
                    "{WIKIS_PREFIX}{}",
                    rest.to_string_lossy().replace('\\', "/")
                );
            }
        }
        path.strip_prefix(self.root.as_path())
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Absolute path of a `rel_path` in either form.
    fn abs(&self, rel_path: &str) -> PathBuf {
        match (&self.wikis, rel_path.strip_prefix(WIKIS_PREFIX)) {
            (Some(wikis), Some(rest)) => wikis.join(rest),
            _ => self.root.join(rel_path),
        }
    }

    /// Lay down `Sermons.base` next to a wiki's sermon folders when it
    /// is not there yet — the reader's table over the collection. Never
    /// rewritten: the views are theirs to shape.
    fn ensure_base(sermons_root: &Path) -> Result<(), ResourcesError> {
        let base = sermons_root.join(SERMONS_BASE);
        if base.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(sermons_root).map_err(|e| ResourcesError::Io(e.to_string()))?;
        std::fs::write(&base, SERMONS_BASE_YAML).map_err(|e| ResourcesError::Io(e.to_string()))
    }

    fn summary(&self, r: &LoadedResource) -> SermonSummary {
        let video = r.resource.media_of("video");
        let (wiki, root) = self.home_of(&r.path);
        let folder = r
            .path
            .parent()
            .and_then(|p| p.strip_prefix(&root).ok())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        SermonSummary {
            slug: r.resource.slug.clone(),
            title: r.resource.title.clone(),
            folder,
            wiki,
            channel: r.resource.writers.first().cloned().unwrap_or_default(),
            video_id: video.map(|m| m.id.clone()).unwrap_or_default(),
            video_url: video.map(|m| m.url.clone()).unwrap_or_default(),
            published: r.resource.published.clone(),
            duration_secs: r.resource.duration_secs,
            tags: r.resource.tags.clone(),
            scripture: r.resource.scripture.clone(),
            rel_path: self.rel(&r.path),
            transcript_rel_path: self.rel(&transcript::transcript_path(&r.path)),
        }
    }

    /// Delete this sermon's previous `sermon-sync` links and mint one
    /// per reference hit. Returns how many links the sermon now has.
    fn replace_links(&self, slug: &str, title: &str, hits: &[RefHit]) -> u32 {
        let Some(store) = &self.links else {
            return 0;
        };
        // `links_for` matches the whole NodeRef, anchor included, so
        // the sermon's timestamped links are found through the graph.
        if let Ok(all) = store.graph(Confidence::Speculative, true) {
            for l in all {
                if l.provenance.source_ref == SOURCE_REF
                    && l.source.kind == NodeKind::Sermon
                    && l.source.id == slug
                {
                    let _ = store.delete(&l.id);
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        let mut count = 0u32;
        for h in hits {
            // One link per (reference, second): the same verse said
            // again a minute later is another moment worth a backlink.
            if !seen.insert((h.osis.clone(), h.secs)) {
                continue;
            }
            let confidence = match (h.spoken, h.chapter_only) {
                (false, false) => Confidence::Likely,
                (false, true) | (true, false) => Confidence::Possible,
                (true, true) => Confidence::Speculative,
            };
            let mut link = TypedLink::new(
                NodeRef::sermon(slug).at(h.secs),
                NodeRef::verse(h.osis.clone()),
                Relation::Mentions,
                confidence,
            );
            // The sermon is an org resource, not a private note.
            link.visibility = Visibility::Unlisted;
            link.provenance.created_by = SOURCE_REF.to_string();
            link.provenance.source_ref = SOURCE_REF.to_string();
            link.provenance.derived = true;
            link.note = format!(
                "{title} · {} — {}",
                sermon::mmss(u64::from(h.secs)),
                h.excerpt
            );
            if store.create(link).is_ok() {
                count += 1;
            }
        }
        count
    }
}

fn io_err(e: &ResourceError) -> ResourcesError {
    ResourcesError::Io(e.to_string())
}

/// The default `Sermons.base`: every sermon manifest under the wiki,
/// newest first, plus a per-channel board.
const SERMONS_BASE_YAML: &str = r#"filters:
  and:
    - resource_kind == "sermon"
properties:
  title:
    displayName: "Sermon"
  published:
    displayName: "Published"
  duration_secs:
    displayName: "Length (s)"
  writers:
    displayName: "Speaker / channel"
  scripture:
    displayName: "Scripture"
views:
  - type: table
    name: "All sermons"
    order:
      - title
      - published
      - duration_secs
      - writers
      - scripture
    sort:
      - property: published
        direction: DESC
  - type: board
    name: "By channel"
    groupBy: writers
"#;

/// Move one file, falling back to copy + remove across filesystems.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
    }
}

/// Reject anything that could climb out of the resources tier.
fn safe_segment(s: &str, what: &str) -> Result<(), ResourcesError> {
    if s.is_empty() || s.contains("..") || s.contains('/') || s.contains('\\') || s.starts_with('.')
    {
        return Err(ResourcesError::BadRequest(format!("{what}: {s:?}")));
    }
    Ok(())
}

impl ResourcesService for ResourcesBackend {
    fn transcript(&self, rel_path: &str) -> Result<TranscriptDoc, ResourcesError> {
        // No traversal outside the resources / wikis trees.
        if rel_path.contains("..") {
            return Err(ResourcesError::NotFound(rel_path.to_string()));
        }
        let mut path = self.abs(rel_path);
        if !path.is_file() {
            // `sermons/<slug>.transcript.json` for a sermon synced into
            // `sermons/<folder>/` — look one directory down.
            let rel = Path::new(rel_path);
            if let (Some(parent), Some(name)) = (rel.parent(), rel.file_name()) {
                let found = std::fs::read_dir(self.abs(&parent.to_string_lossy()))
                    .ok()
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .map(|d| d.join(name))
                    .find(|p| p.is_file());
                if let Some(p) = found {
                    path = p;
                }
            }
        }
        let text = std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ResourcesError::NotFound(rel_path.to_string())
            } else {
                ResourcesError::Io(e.to_string())
            }
        })?;
        serde_json::from_str(&text).map_err(|e| ResourcesError::Io(e.to_string()))
    }

    fn upsert_sermon(&self, sermon: SermonResource) -> Result<SermonUpsert, ResourcesError> {
        safe_segment(&sermon.folder, "folder")?;
        safe_segment(&sermon.video_id, "video_id")?;
        if sermon.title.trim().is_empty() {
            return Err(ResourcesError::BadRequest("title is empty".into()));
        }

        let existing = self.sermons();
        let slugs: Vec<(String, crate::types::Resource)> = existing
            .iter()
            .map(|r| (r.resource.slug.clone(), r.resource.clone()))
            .collect();
        let slug = sermon::slug_for(&slugs, &sermon.video_id, &sermon.title);

        // A known id keeps its file wherever it is (even another
        // folder or wiki); a new one goes into the sync's folder under
        // the wiki it names.
        let md_path = match existing.iter().find(|r| r.resource.slug == slug) {
            Some(r) => r.path.clone(),
            None => {
                let root = self.sermons_root_for(&sermon.wiki)?;
                if !sermon.wiki.is_empty() {
                    Self::ensure_base(&root)?;
                }
                root.join(&sermon.folder).join(format!("{slug}.md"))
            }
        };

        let hits = scripture_refs::extract(&sermon.segments);
        let scripture = scripture_refs::distinct_osis(&hits);

        let (md, created, body_kept) = match std::fs::read_to_string(&md_path) {
            Ok(old) => (
                sermon::refresh_manifest(&old, &sermon, &scripture).map_err(|e| io_err(&e))?,
                false,
                true,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                sermon::render_manifest(&sermon, &slug, &scripture).map_err(|e| io_err(&e))?,
                true,
                false,
            ),
            Err(e) => return Err(ResourcesError::Io(e.to_string())),
        };
        if let Some(parent) = md_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ResourcesError::Io(e.to_string()))?;
        }
        std::fs::write(&md_path, md).map_err(|e| ResourcesError::Io(e.to_string()))?;

        transcript::save(
            transcript::transcript_path(&md_path),
            &sermon::transcript_of(&sermon, &slug),
        )
        .map_err(|e| io_err(&e))?;

        let ann = sidecar::sidecar_path(&md_path);
        if !ann.exists() {
            sidecar::save(&ann, &AnnotationFile::new(&slug)).map_err(|e| io_err(&e))?;
        }

        let links = self.replace_links(&slug, &sermon.title, &hits);

        Ok(SermonUpsert {
            slug,
            rel_path: self.rel(&md_path),
            created,
            body_kept,
            scripture,
            links,
        })
    }

    fn list_sermons(&self) -> Result<Vec<SermonSummary>, ResourcesError> {
        Ok(self.sermons().iter().map(|r| self.summary(r)).collect())
    }

    fn sermon(&self, slug: &str) -> Result<SermonSummary, ResourcesError> {
        self.sermons()
            .iter()
            .find(|r| r.resource.slug == slug)
            .map(|r| self.summary(r))
            .ok_or_else(|| ResourcesError::NotFound(slug.to_string()))
    }

    fn relocate_sermons(&self, folder: &str, wiki: &str) -> Result<u32, ResourcesError> {
        safe_segment(folder, "folder")?;
        if wiki.is_empty() {
            return Err(ResourcesError::BadRequest("wiki is empty".into()));
        }
        let src = self.sermons_root().join(folder);
        if !src.is_dir() {
            return Err(ResourcesError::NotFound(format!("sermons/{folder}")));
        }
        let root = self.sermons_root_for(wiki)?;
        Self::ensure_base(&root)?;
        let dst = root.join(folder);
        std::fs::create_dir_all(&dst).map_err(|e| ResourcesError::Io(e.to_string()))?;
        let mut moved = 0u32;
        let entries = std::fs::read_dir(&src).map_err(|e| ResourcesError::Io(e.to_string()))?;
        for entry in entries.filter_map(Result::ok) {
            let from = entry.path();
            if !from.is_file() {
                continue;
            }
            let Some(name) = from.file_name() else {
                continue;
            };
            move_file(&from, &dst.join(name)).map_err(|e| ResourcesError::Io(e.to_string()))?;
            if from.extension().is_some_and(|e| e == "md") {
                moved += 1;
            }
        }
        // Leave no empty channel folder behind in the org-wide tier.
        let _ = std::fs::remove_dir(&src);
        Ok(moved)
    }

    fn upsert_chart(&self, chart_doc: ChartDoc) -> Result<ChartUpsert, ResourcesError> {
        if chart_doc.title.trim().is_empty() {
            return Err(ResourcesError::BadRequest("title is empty".into()));
        }
        let existing = self.charts();
        let taken: Vec<String> = existing.iter().map(|r| r.resource.slug.clone()).collect();
        let slug = chart::slug_for(&taken, &chart_doc);
        // `slugify` strips every separator, so the slug can never climb
        // out of the tier — but an all-punctuation title yields nothing
        // to name a file with.
        if slug.is_empty() {
            return Err(ResourcesError::BadRequest(format!(
                "title {:?} has no sluggable characters",
                chart_doc.title
            )));
        }

        let root = self.charts_root();
        // A known slug keeps the file it is already in.
        let md_path = existing
            .iter()
            .find(|r| r.resource.slug == slug)
            .map_or_else(|| root.join(format!("{slug}.md")), |r| r.path.clone());
        let (md, created) = match std::fs::read_to_string(&md_path) {
            Ok(old) => (
                chart::refresh_manifest(&old, &chart_doc).map_err(|e| io_err(&e))?,
                false,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                chart::render_manifest(&chart_doc, &slug).map_err(|e| io_err(&e))?,
                true,
            ),
            Err(e) => return Err(ResourcesError::Io(e.to_string())),
        };
        std::fs::create_dir_all(&root).map_err(|e| ResourcesError::Io(e.to_string()))?;
        std::fs::write(&md_path, md).map_err(|e| ResourcesError::Io(e.to_string()))?;
        // The source is the chart: stored verbatim, never re-rendered.
        std::fs::write(chart::source_path(&md_path), &chart_doc.source)
            .map_err(|e| ResourcesError::Io(e.to_string()))?;

        Ok(ChartUpsert {
            slug,
            rel_path: self.rel(&md_path),
            created,
        })
    }

    fn chart(&self, slug: &str) -> Result<ChartDoc, ResourcesError> {
        let found = self
            .charts()
            .into_iter()
            .find(|r| r.resource.slug == slug)
            .ok_or_else(|| ResourcesError::NotFound(slug.to_string()))?;
        let source = match std::fs::read_to_string(chart::source_path(&found.path)) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(ResourcesError::Io(e.to_string())),
        };
        Ok(ChartDoc {
            slug: found.resource.slug,
            title: found.resource.title,
            source,
            key: found.resource.key,
            notation: found.resource.notation,
            sections: found.resource.sections,
            updated_at: found.resource.updated_at,
        })
    }

    fn list_charts(&self) -> Result<Vec<ChartSummary>, ResourcesError> {
        Ok(self
            .charts()
            .iter()
            .map(|r| self.chart_summary(r))
            .collect())
    }

    fn delete_chart(&self, slug: &str) -> Result<bool, ResourcesError> {
        safe_segment(slug, "slug")?;
        let Some(found) = self.charts().into_iter().find(|r| r.resource.slug == slug) else {
            return Ok(false);
        };
        for path in [chart::source_path(&found.path), found.path] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(ResourcesError::Io(e.to_string())),
            }
        }
        Ok(true)
    }

    // ── Signal: patches ──────────────────────────────────────────

    fn upsert_patch(&self, doc: PatchDoc) -> Result<PatchUpsert, ResourcesError> {
        let (slug, md_path) = self.asset_slot(
            PATCHES_DIR,
            ResourceKind::Patch,
            patch::MANIFEST,
            &doc.slug,
            &doc.title,
        )?;
        let (md, created) = match std::fs::read_to_string(&md_path) {
            Ok(old) => (
                patch::refresh_manifest(&old, &doc).map_err(|e| io_err(&e))?,
                false,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                patch::render_manifest(&doc, &slug).map_err(|e| io_err(&e))?,
                true,
            ),
            Err(e) => return Err(ResourcesError::Io(e.to_string())),
        };
        Self::lay_down(&md_path, &md, patch::BODY, &doc.body)?;
        Ok(PatchUpsert {
            slug,
            rel_path: self.rel(&md_path),
            created,
        })
    }

    fn patch(&self, slug: &str) -> Result<PatchDoc, ResourcesError> {
        let found = self
            .asset(PATCHES_DIR, ResourceKind::Patch, slug)
            .ok_or_else(|| ResourcesError::NotFound(slug.to_string()))?;
        Ok(PatchDoc {
            body: Self::read_body(&patch::body_path(&found.path))?,
            content: Self::content_of(&found),
            slug: found.resource.slug,
            title: found.resource.title,
            rig: found.resource.rig,
            tags: found.resource.tags,
            updated_at: found.resource.updated_at,
        })
    }

    fn list_patches(&self) -> Result<Vec<PatchSummary>, ResourcesError> {
        Ok(self
            .assets(PATCHES_DIR, ResourceKind::Patch)
            .iter()
            .map(|r| PatchSummary {
                slug: r.resource.slug.clone(),
                title: r.resource.title.clone(),
                rig: r.resource.rig.clone(),
                tags: r.resource.tags.clone(),
                rel_path: self.rel(&r.path),
                content: Self::content_of(r),
                updated_at: r.resource.updated_at.clone(),
            })
            .collect())
    }

    fn delete_patch(&self, slug: &str) -> Result<bool, ResourcesError> {
        self.delete_asset(PATCHES_DIR, ResourceKind::Patch, slug)
    }

    // ── Signal: samples ──────────────────────────────────────────

    fn upsert_sample(&self, doc: SampleDoc) -> Result<SampleUpsert, ResourcesError> {
        let (slug, md_path) = self.asset_slot(
            SAMPLES_DIR,
            ResourceKind::Sample,
            sample::MANIFEST,
            &doc.slug,
            &doc.title,
        )?;
        let (md, created) = match std::fs::read_to_string(&md_path) {
            Ok(old) => (
                sample::refresh_manifest(&old, &doc).map_err(|e| io_err(&e))?,
                false,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                sample::render_manifest(&doc, &slug).map_err(|e| io_err(&e))?,
                true,
            ),
            Err(e) => return Err(ResourcesError::Io(e.to_string())),
        };
        // Only the metadata is written. The audio was never here.
        Self::lay_down(&md_path, &md, sample::BODY, &doc.body)?;
        Ok(SampleUpsert {
            slug,
            rel_path: self.rel(&md_path),
            created,
        })
    }

    fn sample(&self, slug: &str) -> Result<SampleDoc, ResourcesError> {
        let found = self
            .asset(SAMPLES_DIR, ResourceKind::Sample, slug)
            .ok_or_else(|| ResourcesError::NotFound(slug.to_string()))?;
        Ok(SampleDoc {
            body: Self::read_body(&sample::body_path(&found.path))?,
            content: Self::content_of(&found),
            slug: found.resource.slug,
            title: found.resource.title,
            tags: found.resource.tags,
            duration_secs: found.resource.duration_secs,
            sample_rate: found.resource.sample_rate,
            updated_at: found.resource.updated_at,
        })
    }

    fn list_samples(&self) -> Result<Vec<SampleSummary>, ResourcesError> {
        Ok(self
            .assets(SAMPLES_DIR, ResourceKind::Sample)
            .iter()
            .map(|r| SampleSummary {
                slug: r.resource.slug.clone(),
                title: r.resource.title.clone(),
                tags: r.resource.tags.clone(),
                duration_secs: r.resource.duration_secs,
                sample_rate: r.resource.sample_rate,
                rel_path: self.rel(&r.path),
                content: Self::content_of(r),
                updated_at: r.resource.updated_at.clone(),
            })
            .collect())
    }

    fn delete_sample(&self, slug: &str) -> Result<bool, ResourcesError> {
        self.delete_asset(SAMPLES_DIR, ResourceKind::Sample, slug)
    }

    // ── Ignition: lighting ───────────────────────────────────────

    fn upsert_lighting(&self, doc: LightingDoc) -> Result<LightingUpsert, ResourcesError> {
        // The scope is checked before anything is laid down: a word
        // nobody can act on is refused rather than stored.
        if !lighting::is_scope(&doc.scope) {
            return Err(ResourcesError::BadRequest(format!(
                "scope {:?} is not one of {}",
                doc.scope,
                lighting::SCOPES.join(", ")
            )));
        }
        let (slug, md_path) = self.asset_slot(
            LIGHTING_DIR,
            ResourceKind::Lighting,
            lighting::MANIFEST,
            &doc.slug,
            &doc.title,
        )?;
        let (md, created) = match std::fs::read_to_string(&md_path) {
            Ok(old) => (
                lighting::refresh_manifest(&old, &doc).map_err(|e| io_err(&e))?,
                false,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                lighting::render_manifest(&doc, &slug).map_err(|e| io_err(&e))?,
                true,
            ),
            Err(e) => return Err(ResourcesError::Io(e.to_string())),
        };
        Self::lay_down(&md_path, &md, lighting::BODY, &doc.body)?;
        Ok(LightingUpsert {
            slug,
            rel_path: self.rel(&md_path),
            created,
        })
    }

    fn lighting(&self, slug: &str) -> Result<LightingDoc, ResourcesError> {
        let found = self
            .asset(LIGHTING_DIR, ResourceKind::Lighting, slug)
            .ok_or_else(|| ResourcesError::NotFound(slug.to_string()))?;
        Ok(LightingDoc {
            body: Self::read_body(&lighting::body_path(&found.path))?,
            content: Self::content_of(&found),
            slug: found.resource.slug,
            title: found.resource.title,
            scope: found.resource.scope,
            cues: found.resource.cues,
            updated_at: found.resource.updated_at,
        })
    }

    fn list_lighting(&self) -> Result<Vec<LightingSummary>, ResourcesError> {
        Ok(self
            .assets(LIGHTING_DIR, ResourceKind::Lighting)
            .iter()
            .map(|r| LightingSummary {
                slug: r.resource.slug.clone(),
                title: r.resource.title.clone(),
                scope: r.resource.scope.clone(),
                cues: r.resource.cues.clone(),
                rel_path: self.rel(&r.path),
                content: Self::content_of(r),
                updated_at: r.resource.updated_at.clone(),
            })
            .collect())
    }

    fn delete_lighting(&self, slug: &str) -> Result<bool, ResourcesError> {
        self.delete_asset(LIGHTING_DIR, ResourceKind::Lighting, slug)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sermon(id: &str, title: &str, text: &str) -> SermonResource {
        SermonResource {
            folder: "crossroads".into(),
            wiki: String::new(),
            video_id: id.into(),
            video_url: format!("https://youtu.be/{id}"),
            title: title.into(),
            channel: "Crossroads Church".into(),
            tags: vec!["sermon".into(), "crossroads".into()],
            published: "2026-06-14".into(),
            duration_secs: 120,
            caption_kind: "auto".into(),
            language: "en".into(),
            segments: vec![resources_proto::TranscriptSegment {
                start: 10.0,
                dur: 4.0,
                text: text.into(),
            }],
        }
    }

    #[test]
    fn upsert_lays_down_three_files_and_links() {
        let dir = tempfile::tempdir().unwrap();
        let store = links::Store::open(dir.path().join("links.jsonl"));
        let be = ResourcesBackend::new(dir.path().join("resources")).with_links(store.clone());

        let out = be
            .upsert_sermon(sermon(
                "AAA",
                "God Restores",
                "first Peter chapter five verse seven",
            ))
            .unwrap();
        assert_eq!(out.slug, "god-restores");
        assert_eq!(out.rel_path, "sermons/crossroads/god-restores.md");
        assert!(out.created && !out.body_kept);
        assert_eq!(out.scripture, ["1Pet.5.7"]);
        assert_eq!(out.links, 1);

        let base = dir.path().join("resources/sermons/crossroads");
        assert!(base.join("god-restores.md").is_file());
        assert!(base.join("god-restores.transcript.json").is_file());
        assert!(base.join("god-restores.annotations.json").is_file());

        // The link is anchored at the cue's second and tagged as ours.
        let all = store.graph(Confidence::Speculative, true).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].source.to_token(), "sermon:god-restores#t:10");
        assert_eq!(all[0].target.to_token(), "verse:1Pet.5.7");
        assert_eq!(all[0].provenance.source_ref, SOURCE_REF);

        // Reads: list, one, and the transcript one directory down.
        let list = be.list_sermons().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].folder, "crossroads");
        assert_eq!(list[0].video_id, "AAA");
        assert_eq!(
            list[0].transcript_rel_path,
            "sermons/crossroads/god-restores.transcript.json"
        );
        let doc = be
            .transcript("sermons/god-restores.transcript.json")
            .unwrap();
        assert_eq!(doc.segments.len(), 1);
        assert_eq!(be.sermon("god-restores").unwrap().slug, "god-restores");
    }

    #[test]
    fn resync_keeps_body_and_annotations_and_replaces_links() {
        let dir = tempfile::tempdir().unwrap();
        let store = links::Store::open(dir.path().join("links.jsonl"));
        let be = ResourcesBackend::new(dir.path().join("resources")).with_links(store.clone());
        be.upsert_sermon(sermon("AAA", "God Restores", "John 3:16"))
            .unwrap();

        let base = dir.path().join("resources/sermons/crossroads");
        let md = base.join("god-restores.md");
        let hand = std::fs::read_to_string(&md)
            .unwrap()
            .replace("## Notes", "## Outline\n- `0:00` — Welcome\n\n## Notes");
        std::fs::write(&md, hand).unwrap();
        std::fs::write(
            base.join("god-restores.annotations.json"),
            r#"{"slug":"god-restores","annotations":[{"anchor":"t:9","label":"x","text":"y"}]}"#,
        )
        .unwrap();

        // Renamed video, new captions: same slug, body kept, links replaced.
        let out = be
            .upsert_sermon(sermon(
                "AAA",
                "God Restores (Renamed)",
                "Romans 8 and John 3:16",
            ))
            .unwrap();
        assert_eq!(out.slug, "god-restores");
        assert!(!out.created && out.body_kept);
        assert_eq!(out.scripture, ["Rom.8", "John.3.16"]);
        let text = std::fs::read_to_string(&md).unwrap();
        assert!(text.contains("## Outline\n- `0:00` — Welcome"));
        assert!(
            text.contains("title: God Restores\n"),
            "title is not sync-owned"
        );
        let ann = std::fs::read_to_string(base.join("god-restores.annotations.json")).unwrap();
        assert!(ann.contains("\"t:9\""));
        let all = store.graph(Confidence::Speculative, true).unwrap();
        assert_eq!(all.len(), 2, "old link gone, two new: {all:?}");
    }

    #[test]
    fn a_wiki_sermon_lands_in_the_wikis_resources_with_a_base() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("wikis/bible")).unwrap();
        let store = links::Store::open(dir.path().join("links.jsonl"));
        let be = ResourcesBackend::new(dir.path().join("resources"))
            .with_wikis(dir.path().join("wikis"))
            .with_links(store);

        let mut s = sermon(
            "AAA",
            "God Restores",
            "first Peter chapter five verse seven",
        );
        s.wiki = "bible".into();
        let out = be.upsert_sermon(s.clone()).unwrap();
        assert_eq!(
            out.rel_path,
            "wikis/bible/Resources/Sermons/crossroads/god-restores.md"
        );
        let root = dir.path().join("wikis/bible/Resources/Sermons");
        assert!(root.join("crossroads/god-restores.md").is_file());
        assert!(
            root.join("crossroads/god-restores.transcript.json")
                .is_file()
        );
        assert!(
            root.join(SERMONS_BASE).is_file(),
            "the base is laid down once"
        );
        // The org-wide tier stays empty.
        assert!(!dir.path().join("resources/sermons").exists());

        let listed = be.list_sermons().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].wiki, "bible");
        assert_eq!(listed[0].folder, "crossroads");
        // The transcript resolves through the wikis-relative path.
        let doc = be.transcript(&listed[0].transcript_rel_path).unwrap();
        assert_eq!(doc.segments.len(), 1);

        // A re-sync without `wiki` keeps the file where it is.
        s.wiki.clear();
        let again = be.upsert_sermon(s).unwrap();
        assert_eq!(again.rel_path, out.rel_path);
        assert!(!again.created);

        // An unknown wiki is refused, not created.
        let mut other = sermon("BBB", "Elsewhere", "hello");
        other.wiki = "nope".into();
        assert!(matches!(
            be.upsert_sermon(other),
            Err(ResourcesError::NotFound(_))
        ));
    }

    #[test]
    fn relocate_moves_a_folder_into_the_wiki_and_keeps_slugs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("wikis/bible")).unwrap();
        let store = links::Store::open(dir.path().join("links.jsonl"));
        let be = ResourcesBackend::new(dir.path().join("resources"))
            .with_wikis(dir.path().join("wikis"))
            .with_links(store);
        be.upsert_sermon(sermon("AAA", "God Restores", "John 3:16"))
            .unwrap();
        be.upsert_sermon(sermon("BBB", "Hope", "Romans 8:28"))
            .unwrap();
        assert!(dir.path().join("resources/sermons/crossroads").is_dir());

        let moved = be.relocate_sermons("crossroads", "bible").unwrap();
        assert_eq!(moved, 2);
        assert!(!dir.path().join("resources/sermons/crossroads").exists());
        let dst = dir.path().join("wikis/bible/Resources/Sermons/crossroads");
        assert!(dst.join("god-restores.md").is_file());
        assert!(dst.join("god-restores.transcript.json").is_file());
        assert!(dst.join("god-restores.annotations.json").is_file());
        assert!(
            dir.path()
                .join("wikis/bible/Resources/Sermons")
                .join(SERMONS_BASE)
                .is_file()
        );

        let s = be.sermon("hope").unwrap();
        assert_eq!(s.wiki, "bible");
        assert_eq!(
            s.rel_path,
            "wikis/bible/Resources/Sermons/crossroads/hope.md"
        );
        assert!(be.transcript(&s.transcript_rel_path).is_ok());

        // Gone from the tier, so a second move is a NotFound.
        assert!(matches!(
            be.relocate_sermons("crossroads", "bible"),
            Err(ResourcesError::NotFound(_))
        ));
    }

    #[test]
    fn title_collision_gets_id_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));
        be.upsert_sermon(sermon("AAA", "Hope", "")).unwrap();
        let out = be.upsert_sermon(sermon("BBB", "Hope", "")).unwrap();
        assert_eq!(out.slug, "hope-bbb");
        assert_eq!(be.list_sermons().unwrap().len(), 2);
    }

    fn chart_doc(title: &str, source: &str) -> ChartDoc {
        ChartDoc {
            slug: String::new(),
            title: title.into(),
            source: source.into(),
            key: "A".into(),
            notation: "keyflow".into(),
            sections: vec!["verse-1".into(), "chorus".into()],
            updated_at: "2026-09-05T10:00:00Z".into(),
        }
    }

    /// The two files, the round trip, and the delete — the whole chart
    /// lane against one temp resources tier.
    #[test]
    fn chart_upsert_lays_down_source_and_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));

        let out = be
            .upsert_chart(chart_doc("Great Are You Lord", "[Verse 1]\n| A | E |\n"))
            .unwrap();
        assert_eq!(out.slug, "great-are-you-lord");
        assert_eq!(out.rel_path, "charts/great-are-you-lord.md");
        assert!(out.created);

        let base = dir.path().join("resources/charts");
        assert!(base.join("great-are-you-lord.md").is_file());
        assert_eq!(
            std::fs::read_to_string(base.join("great-are-you-lord.kf")).unwrap(),
            "[Verse 1]\n| A | E |\n",
            "the .kf is the source, verbatim"
        );

        let back = be.chart("great-are-you-lord").unwrap();
        assert_eq!(back.source, "[Verse 1]\n| A | E |\n");
        assert_eq!(back.key, "A");
        assert_eq!(back.sections, ["verse-1", "chorus"]);

        let list = be.list_charts().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].slug, "great-are-you-lord");
        assert_eq!(list[0].notation, "keyflow");

        assert!(be.delete_chart("great-are-you-lord").unwrap());
        assert!(!base.join("great-are-you-lord.md").exists());
        assert!(!base.join("great-are-you-lord.kf").exists());
        assert!(
            !be.delete_chart("great-are-you-lord").unwrap(),
            "deleting twice is `false`, not an error"
        );
        assert!(matches!(
            be.chart("great-are-you-lord"),
            Err(ResourcesError::NotFound(_))
        ));
    }

    /// Two charts titled the same are two charts; naming a slug is how
    /// an app says "the same one again".
    #[test]
    fn chart_slug_collision_makes_a_second_chart() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));
        let first = be.upsert_chart(chart_doc("Hosanna", "| A |")).unwrap();
        let second = be.upsert_chart(chart_doc("Hosanna", "| E |")).unwrap();
        assert_eq!(first.slug, "hosanna");
        assert_eq!(second.slug, "hosanna-2");
        assert!(second.created);
        assert_eq!(be.list_charts().unwrap().len(), 2);

        let mut again = chart_doc("Hosanna (Live)", "| D |");
        again.slug = "hosanna".into();
        let update = be.upsert_chart(again).unwrap();
        assert_eq!(update.slug, "hosanna");
        assert!(!update.created, "a named slug updates rather than forks");
        assert_eq!(be.chart("hosanna").unwrap().source, "| D |");
        assert_eq!(be.list_charts().unwrap().len(), 2);
    }

    #[test]
    fn chart_re_upsert_keeps_the_manifest_body() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));
        be.upsert_chart(chart_doc("Hosanna", "| A |")).unwrap();
        let md = dir.path().join("resources/charts/hosanna.md");
        let hand = std::fs::read_to_string(&md)
            .unwrap()
            .replace("## Notes", "## Notes\n- capo 2, drop the bridge\n");
        std::fs::write(&md, hand).unwrap();

        let mut next = chart_doc("Hosanna", "| A | E |");
        next.slug = "hosanna".into();
        next.key = "E".into();
        be.upsert_chart(next).unwrap();

        let text = std::fs::read_to_string(&md).unwrap();
        assert!(text.contains("- capo 2, drop the bridge"), "{text}");
        assert!(text.contains("key: E"), "app-owned key rewritten: {text}");
        assert_eq!(be.chart("hosanna").unwrap().source, "| A | E |");
    }

    #[test]
    fn chart_without_a_title_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));
        assert!(matches!(
            be.upsert_chart(chart_doc("   ", "| A |")),
            Err(ResourcesError::BadRequest(_))
        ));
        // A title that kebabs to nothing has no file to be, either.
        assert!(matches!(
            be.upsert_chart(chart_doc("—", "| A |")),
            Err(ResourcesError::BadRequest(_))
        ));
        assert!(be.list_charts().unwrap().is_empty());
    }

    // ── The three asset lanes ────────────────────────────────────

    fn patch_doc(title: &str, body: &str) -> PatchDoc {
        PatchDoc {
            slug: String::new(),
            title: title.into(),
            rig: "helix".into(),
            tags: vec!["pad".into()],
            body: body.into(),
            content: ContentRef::default(),
            updated_at: "2026-09-05T10:00:00Z".into(),
        }
    }

    fn sample_doc(title: &str, body: &str) -> SampleDoc {
        SampleDoc {
            slug: String::new(),
            title: title.into(),
            tags: vec!["kick".into()],
            duration_secs: 2,
            sample_rate: 48_000,
            body: body.into(),
            content: ContentRef {
                root_id: "acme-library".into(),
                path: "Samples/Kicks/Room Kick.wav".into(),
            },
            updated_at: "2026-09-05T10:00:00Z".into(),
        }
    }

    fn lighting_doc(title: &str, body: &str) -> LightingDoc {
        LightingDoc {
            slug: String::new(),
            title: title.into(),
            scope: "setlist".into(),
            cues: vec!["12".into(), "13".into()],
            body: body.into(),
            content: ContentRef::default(),
            updated_at: "2026-09-05T10:00:00Z".into(),
        }
    }

    /// The whole patch lane against one temp tier: a directory per
    /// patch, the body verbatim, the round trip, and the delete.
    #[test]
    fn patch_upsert_lays_down_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));

        let out = be
            .upsert_patch(patch_doc("Warm Analog Pad", "{\"blocks\":[\"reverb\"]}"))
            .unwrap();
        assert_eq!(out.slug, "warm-analog-pad");
        assert_eq!(out.rel_path, "patches/warm-analog-pad/patch.md");
        assert!(out.created);

        let base = dir.path().join("resources/patches/warm-analog-pad");
        assert!(base.join("patch.md").is_file());
        assert_eq!(
            std::fs::read_to_string(base.join("patch.json")).unwrap(),
            "{\"blocks\":[\"reverb\"]}",
            "the definition is stored verbatim"
        );

        let back = be.patch("warm-analog-pad").unwrap();
        assert_eq!(back.body, "{\"blocks\":[\"reverb\"]}");
        assert_eq!(back.rig, "helix");
        assert_eq!(back.tags, ["pad"]);

        let list = be.list_patches().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].slug, "warm-analog-pad");

        // A sidecar a person dropped beside the patch goes with it.
        std::fs::write(base.join("pedalboard.jpg"), b"jpeg").unwrap();
        assert!(be.delete_patch("warm-analog-pad").unwrap());
        assert!(!base.exists(), "the directory is the unit");
        assert!(
            !be.delete_patch("warm-analog-pad").unwrap(),
            "deleting twice is `false`, not an error"
        );
    }

    /// Two patches titled the same are two patches; naming a slug is
    /// how an app says "the same one again". Same rule as the chart
    /// lane, because it is the same rule.
    #[test]
    fn patch_slug_collision_makes_a_second_patch() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));
        assert_eq!(
            be.upsert_patch(patch_doc("Lead", "{}")).unwrap().slug,
            "lead"
        );
        let second = be.upsert_patch(patch_doc("Lead", "{\"gain\":1}")).unwrap();
        assert_eq!(second.slug, "lead-2");
        assert_eq!(be.list_patches().unwrap().len(), 2);

        let mut again = patch_doc("Lead (Bright)", "{\"gain\":2}");
        again.slug = "lead".into();
        let update = be.upsert_patch(again).unwrap();
        assert_eq!(update.slug, "lead");
        assert!(!update.created, "a named slug updates rather than forks");
        assert_eq!(be.patch("lead").unwrap().body, "{\"gain\":2}");
        assert_eq!(be.list_patches().unwrap().len(), 2);
    }

    #[test]
    fn patch_re_upsert_keeps_the_manifest_body() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));
        be.upsert_patch(patch_doc("Lead", "{}")).unwrap();
        let md = dir.path().join("resources/patches/lead/patch.md");
        let hand = std::fs::read_to_string(&md)
            .unwrap()
            .replace("## Notes", "## Notes\n- bridge pickup only\n");
        std::fs::write(&md, hand).unwrap();

        let mut next = patch_doc("Lead", "{\"gain\":3}");
        next.slug = "lead".into();
        next.rig = "kemper".into();
        be.upsert_patch(next).unwrap();

        let text = std::fs::read_to_string(&md).unwrap();
        assert!(text.contains("- bridge pickup only"), "{text}");
        assert!(
            text.contains("rig: kemper"),
            "app-owned rig rewritten: {text}"
        );
        assert_eq!(be.patch("lead").unwrap().body, "{\"gain\":3}");
    }

    /// The sample lane's whole point: the manifest is the sample, and
    /// the audio is somewhere else entirely.
    #[test]
    fn sample_upsert_records_where_the_audio_is_without_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));

        let out = be
            .upsert_sample(sample_doc("Room Kick 48k", "{\"mic\":\"D112\"}"))
            .unwrap();
        assert_eq!(out.slug, "room-kick-48k");
        assert_eq!(out.rel_path, "samples/room-kick-48k/sample.md");

        let base = dir.path().join("resources/samples/room-kick-48k");
        // Two files, and neither of them is audio.
        let mut names: Vec<String> = std::fs::read_dir(&base)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["sample.json", "sample.md"]);

        let back = be.sample("room-kick-48k").unwrap();
        assert_eq!(back.duration_secs, 2);
        assert_eq!(back.sample_rate, 48_000);
        assert_eq!(back.body, "{\"mic\":\"D112\"}");
        assert_eq!(back.content.root_id, "acme-library");
        assert_eq!(back.content.path, "Samples/Kicks/Room Kick.wav");
        assert!(back.content.is_bound());

        // The list carries the binding too, so a library view can say
        // which rows are playable without a read per row.
        let list = be.list_samples().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].content.root_id, "acme-library");

        assert!(be.delete_sample("room-kick-48k").unwrap());
        assert!(!base.exists());
    }

    /// A declared sample with no bytes yet is the ordinary state, not
    /// an error.
    #[test]
    fn a_sample_may_have_no_bytes_bound_yet() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));
        let mut doc = sample_doc("Unbound", "");
        doc.content = ContentRef::default();
        be.upsert_sample(doc).unwrap();
        let back = be.sample("unbound").unwrap();
        assert!(!back.content.is_bound());
        assert_eq!(back.content.root_id, "");
    }

    #[test]
    fn lighting_upsert_lays_down_a_directory_and_validates_its_scope() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));

        let out = be
            .upsert_lighting(lighting_doc("Sunday Set", "{\"cues\":[]}"))
            .unwrap();
        assert_eq!(out.slug, "sunday-set");
        assert_eq!(out.rel_path, "lighting/sunday-set/show.md");

        let base = dir.path().join("resources/lighting/sunday-set");
        assert_eq!(
            std::fs::read_to_string(base.join("show.json")).unwrap(),
            "{\"cues\":[]}"
        );

        let back = be.lighting("sunday-set").unwrap();
        assert_eq!(back.scope, "setlist");
        assert_eq!(back.cues, ["12", "13"], "the cues an anchor may address");

        assert_eq!(be.list_lighting().unwrap().len(), 1);
        assert!(be.delete_lighting("sunday-set").unwrap());
        assert!(!base.exists());
    }

    /// A scope outside the vocabulary is refused, and nothing is
    /// written — a word nobody can act on is worse than no word.
    #[test]
    fn an_unknown_lighting_scope_is_refused_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));
        for scope in ["", "evening", "Show", "tour"] {
            let mut doc = lighting_doc("Sunday Set", "{}");
            doc.scope = scope.into();
            assert!(
                matches!(be.upsert_lighting(doc), Err(ResourcesError::BadRequest(_))),
                "scope {scope:?} must be refused"
            );
        }
        assert!(be.list_lighting().unwrap().is_empty());
        assert!(!dir.path().join("resources/lighting").exists());

        // And each of the three is accepted.
        for scope in lighting::SCOPES {
            let mut doc = lighting_doc(&format!("Set {scope}"), "{}");
            doc.scope = (*scope).into();
            assert!(be.upsert_lighting(doc).is_ok(), "{scope} is a scope");
        }
        assert_eq!(be.list_lighting().unwrap().len(), 3);
    }

    /// The same refusal in all three lanes: an empty title, and a title
    /// with nothing sluggable in it, name no file.
    #[test]
    fn an_asset_without_a_usable_title_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));
        for title in ["   ", "—"] {
            assert!(matches!(
                be.upsert_patch(patch_doc(title, "{}")),
                Err(ResourcesError::BadRequest(_))
            ));
            assert!(matches!(
                be.upsert_sample(sample_doc(title, "{}")),
                Err(ResourcesError::BadRequest(_))
            ));
            assert!(matches!(
                be.upsert_lighting(lighting_doc(title, "{}")),
                Err(ResourcesError::BadRequest(_))
            ));
        }
        assert!(be.list_patches().unwrap().is_empty());
        assert!(be.list_samples().unwrap().is_empty());
        assert!(be.list_lighting().unwrap().is_empty());
    }

    /// The lanes do not see each other: three kinds under one tier,
    /// each listing only its own.
    #[test]
    fn the_lanes_do_not_see_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));
        be.upsert_patch(patch_doc("Lead", "{}")).unwrap();
        be.upsert_sample(sample_doc("Room Kick", "{}")).unwrap();
        be.upsert_lighting(lighting_doc("Sunday Set", "{}"))
            .unwrap();
        be.upsert_chart(chart_doc("Hosanna", "| A |")).unwrap();

        assert_eq!(be.list_patches().unwrap().len(), 1);
        assert_eq!(be.list_samples().unwrap().len(), 1);
        assert_eq!(be.list_lighting().unwrap().len(), 1);
        assert_eq!(be.list_charts().unwrap().len(), 1);
        assert!(matches!(
            be.patch("room-kick"),
            Err(ResourcesError::NotFound(_))
        ));
    }

    /// A slug that tries to climb out of its lane is refused before
    /// anything is removed.
    #[test]
    fn delete_refuses_a_traversing_slug() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));
        assert!(matches!(
            be.delete_patch("../charts"),
            Err(ResourcesError::BadRequest(_))
        ));
    }

    #[test]
    fn rejects_unsafe_folder() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path());
        let mut s = sermon("AAA", "x", "");
        s.folder = "../etc".into();
        assert!(matches!(
            be.upsert_sermon(s),
            Err(ResourcesError::BadRequest(_))
        ));
    }
}
