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
    ResourcesError, ResourcesService, SermonResource, SermonSummary, SermonUpsert, TranscriptDoc,
};

use crate::scripture_refs::{self, RefHit};
use crate::types::AnnotationFile;
use crate::walker::{LoadedResource, walk};
use crate::{ResourceError, sermon, sidecar, transcript};

/// `provenance.source_ref` on every link the sync mints — so a re-sync
/// replaces only its own links, never a reader's annotations.
pub const SOURCE_REF: &str = "sermon-sync";

/// The subtree sermons live in, under the resources root.
const SERMONS_DIR: &str = "sermons";

#[derive(Clone, architect::HasDispatcher)]
pub struct ResourcesBackend {
    /// `<org>/resources`.
    root: Arc<PathBuf>,
    /// The org's typed-link store, when the host wires one in.
    links: Option<links::Store>,
}

impl ResourcesBackend {
    #[must_use]
    pub fn new(resources_root: impl Into<PathBuf>) -> Self {
        Self {
            root: Arc::new(resources_root.into()),
            links: None,
        }
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

    /// Every sermon manifest under `sermons/**` as `(slug, resource,
    /// absolute path)`.
    fn sermons(&self) -> Vec<LoadedResource> {
        walk(self.sermons_root())
            .into_iter()
            .filter(|r| r.resource.kind == crate::types::ResourceKind::Sermon)
            .collect()
    }

    fn rel(&self, path: &Path) -> String {
        path.strip_prefix(self.root.as_path())
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn summary(&self, r: &LoadedResource) -> SermonSummary {
        let video = r.resource.media_of("video");
        let folder = r
            .path
            .parent()
            .and_then(|p| p.strip_prefix(self.sermons_root()).ok())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        SermonSummary {
            slug: r.resource.slug.clone(),
            title: r.resource.title.clone(),
            folder,
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
        // No traversal outside the resources tier.
        if rel_path.contains("..") {
            return Err(ResourcesError::NotFound(rel_path.to_string()));
        }
        let mut path = self.root.join(rel_path);
        if !path.is_file() {
            // `sermons/<slug>.transcript.json` for a sermon synced into
            // `sermons/<folder>/` — look one directory down.
            let rel = Path::new(rel_path);
            if let (Some(parent), Some(name)) = (rel.parent(), rel.file_name()) {
                let found = std::fs::read_dir(self.root.join(parent))
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
        // folder); a new one goes into the sync's folder.
        let md_path = existing
            .iter()
            .find(|r| r.resource.slug == slug)
            .map(|r| r.path.clone())
            .unwrap_or_else(|| {
                self.sermons_root()
                    .join(&sermon.folder)
                    .join(format!("{slug}.md"))
            });

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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sermon(id: &str, title: &str, text: &str) -> SermonResource {
        SermonResource {
            folder: "crossroads".into(),
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
    fn title_collision_gets_id_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let be = ResourcesBackend::new(dir.path().join("resources"));
        be.upsert_sermon(sermon("AAA", "Hope", "")).unwrap();
        let out = be.upsert_sermon(sermon("BBB", "Hope", "")).unwrap();
        assert_eq!(out.slug, "hope-bbb");
        assert_eq!(be.list_sermons().unwrap().len(), 2);
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
