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
    SampleSummary, SampleUpsert, SermonResource, SermonSummary, SermonUpsert, SongDoc, SongSummary,
    SongUpsert, TranscriptDoc,
};

use vault_proto::{IfMatch, VaultSync as _};

use crate::scripture_refs::{self, RefHit};
use crate::types::{AnnotationFile, ResourceKind};
use crate::walker::{LoadedResource, walk};
use crate::{ResourceError, chart, lighting, patch, sample, sermon, sidecar, song, transcript};

/// The subtree song folders and song media share
/// (`<org>/resources/songs/`).
///
/// Both meanings of "song" live here and only one of them moved: the
/// markdown (`song.md`, `arrangements/**`) is now on the vault's Assets
/// shelf; `manifest.json` and the audio stems stay, because they are
/// imports the `/media` route serves. See
/// [`ResourcesBackend::migrate_song_folders`].
pub const LEGACY_SONGS_DIR: &str = "songs";

/// The vendored `song` crate's index file inside a song folder. Its
/// presence is what tells a *song folder* from the media directory of
/// the same name.
const VENDORED_SONG_FILE: &str = "song.md";

/// The vendored `song` crate's arrangements subdirectory.
const VENDORED_ARRANGEMENTS_DIR: &str = "arrangements";

/// The vendored `song` crate's per-arrangement note.
const VENDORED_ARRANGEMENT_FILE: &str = "arrangement.md";

/// A top-level scalar from a vendored frontmatter block, by key.
///
/// Hand-rolled rather than `serde_yaml` because the vendored files
/// carry nested structures this does not model and must not have to:
/// the migration reads four scalars off them (`title`, `name`, `key`,
/// `id`, `defaultArrangement`) and copies the rest of the document
/// verbatim. Parsing a shape defined in another repository, at a
/// pinned tag, would make this code break when that repository changed
/// something the migration does not care about.
fn vendored_key(document: &str, key: &str) -> Option<String> {
    let (fm, _) = crate::sermon::split(document)?;
    for line in fm.lines() {
        // Top-level only — a nested `  name:` under `arrangements:`
        // belongs to an entry, not to the document.
        if line.starts_with(char::is_whitespace) || line.starts_with('-') {
            continue;
        }
        if let Some(rest) = line.strip_prefix(key).and_then(|r| r.strip_prefix(':')) {
            let v = rest.trim().trim_matches(['"', '\'']);
            return (!v.is_empty()).then(|| v.to_owned());
        }
    }
    None
}

/// The breadcrumb left in `<org>/resources/songs/` after a migration.
const SONG_MIGRATION_NOTE_BODY: &str = "\
# The song documents moved to the assets tier

ADR 0004 decision 1. A song's **document** is now a document on an asset
group of its own, and so is each of its arrangements:

    <org>/assets/songs/<slug>.md      — the song
    <org>/assets/charts/<slug>.md     — one per arrangement

That is what buys them collaborative editing, wikilinks, tags and
search — an asset group is registered for per-file CRDT exactly as a
wiki is — *and* what makes them reachable from another organisation: a
group is a shelf, and a shelf can be subscribed to. Each arrangement
became a chart that *names* its song (`song: song:<slug>`) instead of
being nested inside it, and the `defaultArrangement` uuid became
`is_default` on the chart it pointed at — the flag now lives on the
thing it is a fact about.

**What is still live here, and must not be deleted:** `manifest.json`
and the audio stems. Those are Resources — imported bytes nobody types
into — and `GET /org/{slug}/media/songs/...` serves them to the player.
They did not move and are not a snapshot. One song, sorted into two
tiers by whether anybody types into it.

**What is a frozen snapshot:** `song.md` and `arrangements/**`. They
were copied, not moved, so a reader that has not been repointed still
finds something. The live copy is the one on the shelf.
";

/// The breadcrumb left in `<org>/resources/charts/` after a migration,
/// for the person who opens the folder later and finds files nothing
/// reads.
const MIGRATION_NOTE: &str = "_MIGRATED.md";

/// What that breadcrumb says. Deliberately prose rather than a marker
/// file: the reader is a human wondering whether it is safe to delete
/// the folder, and the honest answer has a condition in it.
const MIGRATION_NOTE_BODY: &str = "\
# These charts moved to the assets tier

ADR 0004 decision 1 made charts documents on an asset group of their
own:

    <org>/assets/charts/<slug>.md

Each of those holds what used to be two files here — the manifest's
frontmatter, and the `.kf` source, now a ```keyflow fence in the body.
That is what buys a chart collaborative editing, wikilinks, tags and
search, none of which this tier could offer.

**The files beside this note were copied, not moved, and nothing reads
them any more.** They are a snapshot frozen at migration time: edits
made on the shelf since are not reflected here.

They are deliberately not deleted, because a subscriber taking this
org's `charts` library before the move holds a copy addressed against
this directory, and deleting the original under a live subscription is
not something a migration should decide on its own. Cross-organisation
resolution reads the shelf first (`node_homes::LocalHomes::locate`), so
a foreign `chart:<slug>` already follows the live copy. Once no
subscriber is pointed here, this directory can go.
";

/// What one run of [`ResourcesBackend::migrate_charts`] did — named
/// slugs rather than counts, because the thing an operator wants from a
/// migration log is *which* chart, and a count cannot answer that.
#[derive(Debug, Default, Clone)]
pub struct ChartMigration {
    /// Charts copied onto the shelf by this run.
    pub migrated: Vec<String>,
    /// Charts already on the shelf, left alone. The steady state.
    pub kept: Vec<String>,
}

/// `provenance.source_ref` on every link the sync mints — so a re-sync
/// replaces only its own links, never a reader's annotations.
pub const SOURCE_REF: &str = "sermon-sync";

/// The subtree sermons live in, under the org-wide resources root.
const SERMONS_DIR: &str = "sermons";

/// The subtree charts lived in under the org-wide resources root
/// (ADR 0003: `resources/charts/<slug>.kf`).
///
/// **Read-only as of ADR 0004.** Charts are shelf documents now
/// ([`AssetShelf`]); this constant survives so
/// [`ResourcesBackend::migrate_charts`] can find what ADR 0003 left
/// behind, and so the migration's breadcrumb lands in the right place.
pub const LEGACY_CHARTS_DIR: &str = "charts";

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

/// One **asset group** — `<org>/assets/<kind>/`, registered on the
/// vault backend as `assets:<kind>` (ADR 0004 decision 1).
///
/// # Why the backend holds a shelf and not a directory
///
/// This is the whole mechanical content of "charts become collaborative
/// documents". A chart write is a [`VaultSync::put_file`], which is
/// what makes `vault-collab` see it: the backend takes its write lock,
/// hashes the bytes, and broadcasts a `VaultEvent::Put` that the
/// per-shelf inbound listener folds into any open Loro document for
/// that path. Write around it — a bare `std::fs::write`, which is
/// exactly what ADR 0003's chart lane did — and a chart somebody has
/// open in another tab silently reverts on their next keystroke,
/// because the doc never learned the file changed.
///
/// **Nothing here needs the shelf to be inside the vault**, and that is
/// the correction this type carries. An earlier draft filed charts at
/// `<vault>/Assets/Charts/` to obtain the paragraph above; every word
/// of it is true of any root the server registered, which is why the
/// wiki tier has been collaborative all along from outside the vault.
/// The shelf is a sibling now, and the only difference on this side is
/// which `vault_id` the write names. What changed on the *other* side
/// is that a sibling can be subscribed to and a vault cannot.
///
/// One of these per kind, because a kind is the shelf: the charts group
/// and the songs group are separate roots with separate ids, so a path
/// on one is `<slug>.md` rather than `Assets/Charts/<slug>.md`.
///
/// Reads deliberately go straight to disk instead. Listing charts means
/// parsing frontmatter out of every document on the shelf, and no wire
/// call returns that for a subtree; `manifest` returns paths and shas.
/// Reading around the backend costs nothing — there is no lock to take
/// and no event to emit — so the asymmetry is the right one rather than
/// an inconsistency.
#[derive(Clone)]
struct AssetShelf {
    /// The backend every asset write goes through.
    vault: vault::sync::Backend,
    /// This group's id on that backend — `assets:charts`,
    /// `assets:songs`. Composed by `org_proto::Shelf::vault_id`, so it
    /// is the same string the boot loop registered.
    vault_id: String,
    /// This group's root on disk, resolved once — the walk side of the
    /// asymmetry above.
    root: PathBuf,
}

impl AssetShelf {
    /// A shelf-relative, forward-slashed path for an absolute one under
    /// the root. This is what an app gets back as `rel_path`, and what
    /// it hands to `VaultSync` to open the document — so it must be the
    /// shelf's own spelling, not the resources tier's.
    fn rel(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Write one asset document.
    ///
    /// [`IfMatch::Force`], and the reason is the conflict policy
    /// `vault-collab` states canonically rather than laziness. The
    /// chart lane's contract is that the slug is the identity and a
    /// save replaces the source outright; there is no sha for Keyflow
    /// to have held, because Keyflow addresses charts by slug and never
    /// saw one. A conditional write would therefore have nothing to
    /// condition on and would fail whenever *anyone* — including the
    /// collab write-behind flushing a keystroke a second ago — had
    /// touched the file.
    ///
    /// Forcing is safe here precisely because the doc layer is
    /// downstream of it: an external `put_file` into an open document
    /// is merged in character by character against the last flushed
    /// text, so concurrent typing interleaves with the save rather than
    /// being reverted. Last-writer-wins at the file, three-way merge at
    /// the document. That is the documented behaviour, not a happy
    /// accident, and it is why the app-owned/authored split in
    /// [`crate::chart::refresh_document`] matters: the smaller the
    /// region a save rewrites, the less there is to merge.
    fn put(&self, rel: &str, text: &str) -> Result<(), ResourcesError> {
        self.vault
            .put_file(
                &self.vault_id,
                rel,
                text.as_bytes().to_vec(),
                IfMatch::Force,
            )
            .map(|_ack| ())
            .map_err(|e| ResourcesError::Io(format!("write {rel}: {e}")))
    }

    /// Remove one asset document, idempotently.
    fn delete(&self, rel: &str) -> Result<(), ResourcesError> {
        self.vault
            .delete_file(&self.vault_id, rel, IfMatch::Force)
            .map_err(|e| ResourcesError::Io(format!("delete {rel}: {e}")))
    }
}

#[derive(Clone, architect::HasDispatcher)]
pub struct ResourcesBackend {
    /// `<org>/resources`.
    root: Arc<PathBuf>,
    /// The org's vault, when the host wires one in — ADR 0004's Assets
    /// tier. The chart lane refuses without it rather than falling back
    /// to ADR 0003's `std::fs` path: two write paths for one lane is
    /// how a "collaborative" chart quietly stops being one, and a lane
    /// that fails loudly on a misconfigured host is cheaper to find
    /// than a lane that silently writes the wrong tier.
    assets: std::collections::BTreeMap<String, AssetShelf>,
    /// Serialises the read-decide-rewrite the chart default invariant
    /// needs (see [`ResourcesBackend::reconcile_song_defaults`]).
    ///
    /// The invariant is a statement about a *set* of files, so the
    /// decision is only sound if nobody else is editing that set
    /// between the read and the writes. Two Keyflow tabs both saving an
    /// arrangement of one song is the ordinary case, and without this
    /// they interleave into a song with two defaults — or none.
    /// Clones share the lock, which is what makes it hold across the
    /// per-request clones the RPC layer hands out.
    chart_defaults: Arc<std::sync::Mutex<()>>,
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
            assets: std::collections::BTreeMap::new(),
            wikis: None,
            links: None,
            chart_defaults: Arc::new(std::sync::Mutex::new(())),
        }
    }

    /// Attach the org's vault, so the chart lane can write the Assets
    /// tier (ADR 0004 decision 1).
    ///
    /// Without this the chart RPCs refuse; see [`ResourcesBackend::assets`].
    ///
    /// # Errors
    ///
    /// [`ResourcesError::Io`] when `vault_id` names no registered root.
    pub fn with_assets(mut self, vault: vault::sync::Backend) -> Result<Self, ResourcesError> {
        use resources_proto::assets::{CHARTS_KIND, SONGS_KIND};
        for (kind, vault_id) in [
            (CHARTS_KIND, resources_proto::assets::charts_vault_id()),
            (SONGS_KIND, resources_proto::assets::songs_vault_id()),
        ] {
            let root = vault
                .root(&vault_id)
                .map_err(|e| ResourcesError::Io(format!("asset shelf `{vault_id}`: {e}")))?;
            self.assets.insert(
                kind.to_owned(),
                AssetShelf {
                    vault: vault.clone(),
                    vault_id,
                    root,
                },
            );
        }
        Ok(self)
    }

    /// Move every ADR 0003 chart off `<org>/resources/charts/` onto the
    /// charts asset group. Idempotent, and it deletes nothing.
    ///
    /// Run on every boot. There is real data on production —
    /// `chart:doxology`, `chart:doxology-2`, `chart:agent-smoke-test`
    /// and `chart:vox` in org `codywright` — and a migration that has
    /// to be remembered is a migration that gets skipped on the one
    /// deployment that mattered.
    ///
    /// # Copy, never move
    ///
    /// Each legacy pair (`<slug>.md` + `<slug>.kf`) is *read* and
    /// composed into `<org>/assets/charts/<slug>.md` through
    /// [`AssetShelf::put`]. The originals stay exactly where they are.
    /// Three reasons, in order of how much they cost if ignored:
    ///
    /// 1. **Reversible by inspection.** Both copies are on disk and a
    ///    person can diff them. Nothing about the rollback story
    ///    depends on a backup existing or on this code being correct.
    /// 2. **Cross-organisation reach still reads the old tier.**
    ///    `node_homes::LocalHomes::locate` and the
    ///    `SourceKind::Resource` materialiser both resolve a foreign
    ///    `chart:` through `<org>/resources/charts/`. Deleting the
    ///    originals would break a subscriber's library the moment this
    ///    shipped. They keep working — against a snapshot frozen at
    ///    migration time, which is a regression and is documented as
    ///    one rather than hidden by a deletion.
    /// 3. **A half-finished migration is not a lost chart.** If this
    ///    process dies between two charts, the next boot resumes and
    ///    the untouched ones are still where they always were.
    ///
    /// A `_MIGRATED.md` breadcrumb is written beside the originals
    /// saying where they went and that they are no longer read. That
    /// note is for the person who opens the folder in six months, which
    /// is the only reader a leftover directory ever has.
    ///
    /// # Idempotence
    ///
    /// A slug whose asset document already exists is skipped outright —
    /// the vault copy is authoritative the instant it exists, and
    /// re-composing it from the frozen originals would revert every
    /// edit made since. Skipped is the *normal* outcome: on a settled
    /// deployment every boot after the first migrates nothing.
    ///
    /// # Errors
    ///
    /// The first unreadable legacy chart or unwritable vault path.
    /// Charts already migrated are still counted, so a retry after a
    /// fixed permission resumes rather than restarts.
    pub fn migrate_charts(&self) -> Result<ChartMigration, ResourcesError> {
        let shelf = self.shelf(resources_proto::assets::CHARTS_KIND)?;
        let legacy_dir = self.root.join(LEGACY_CHARTS_DIR);
        let mut report = ChartMigration::default();
        // Only ADR 0003's own manifests: `walk` parses frontmatter, so
        // a stray note in the folder is not mistaken for a chart.
        for found in walk(&legacy_dir)
            .into_iter()
            .filter(|r| r.resource.kind == crate::types::ResourceKind::Chart)
        {
            let slug = found.resource.slug.clone();
            let rel = resources_proto::assets::chart_path(&slug);
            if shelf.root.join(&rel).exists() {
                report.kept.push(slug);
                continue;
            }
            let manifest = std::fs::read_to_string(&found.path)
                .map_err(|e| ResourcesError::Io(format!("read {}: {e}", found.path.display())))?;
            // A `.kf` that is gone is an empty source, not a failure:
            // the manifest is the chart's identity and losing it is
            // what would be unrecoverable.
            let source = match std::fs::read_to_string(found.path.with_extension("kf")) {
                Ok(s) => s,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(ResourcesError::Io(e.to_string())),
            };
            let document = chart::migrate_document(&manifest, &source).map_err(|e| io_err(&e))?;
            shelf.put(&rel, &document)?;
            report.migrated.push(slug);
        }
        if !report.migrated.is_empty() {
            let note = legacy_dir.join(MIGRATION_NOTE);
            if !note.exists() {
                let _ = std::fs::write(&note, MIGRATION_NOTE_BODY);
            }
        }
        self.migrate_song_folders(&mut report)?;
        Ok(report)
    }

    /// The other half of the migration: the vendored song folders
    /// `task song add` wrote — `<resources>/songs/<slug>/song.md` plus
    /// `arrangements/<dir>/{arrangement.md,*.kf}` — become a song
    /// document and one chart per arrangement.
    ///
    /// # Two things called "songs", and only one of them moves
    ///
    /// `<resources>/songs/<slug>/` holds both. `manifest.json` and the
    /// audio stems beside it are **Resources** in ADR 0004's sense —
    /// imports, binary, nothing anybody types into — and they stay
    /// exactly where they are, which is why a cross-organisation
    /// `song:<slug>` keeps resolving *and* keeps being fetchable where
    /// a `chart:<slug>` no longer is. What moves is the markdown: the
    /// document a person writes, edits and links to.
    ///
    /// That the two shared a directory was an accident of history, and
    /// separating them by *file kind* rather than by directory is
    /// exactly the tier rule stated as code.
    ///
    /// # Translating the vendored shape
    ///
    /// Each `arrangements/<dir>/arrangement.md` becomes a chart whose
    /// `song` is `song:<song-slug>`, whose `arrangement` is the
    /// vendored `name`, and whose source is the `.kf` that sat beside
    /// it — folded into the chart's own fence. The song's
    /// `defaultArrangement` uuid selects which chart gets `is_default`,
    /// and then stops existing: the flag lives on the thing it is a
    /// fact about, where `reconcile_song_defaults` can maintain it.
    ///
    /// A song's *first or only* arrangement takes the song's own slug
    /// (`chart:opening-night`), so the common case reads the way a
    /// person would name it; a second one is suffixed by its label
    /// through the ordinary [`chart::slug_for`] path.
    ///
    /// # What still reads the originals
    ///
    /// The global player fetches
    /// `GET /org/{org}/media/songs/{slug}/song.md` and the
    /// `arrangement.md` it points at (`player_ui::song_session`). Those
    /// files are **copied, not moved**, so it keeps working — against a
    /// snapshot frozen at migration time. Repointing the player at the
    /// Assets tier needs a cross-tier read path the vault does not
    /// have; it is recorded in `docs/spec/unmet.md` beside the
    /// cross-org gap rather than left to be discovered.
    fn migrate_song_folders(&self, report: &mut ChartMigration) -> Result<(), ResourcesError> {
        let shelf = self.shelf(resources_proto::assets::SONGS_KIND)?;
        let songs_dir = self.root.join(LEGACY_SONGS_DIR);
        let Ok(entries) = std::fs::read_dir(&songs_dir) else {
            return Ok(());
        };
        let mut touched = false;
        for entry in entries.flatten() {
            let dir = entry.path();
            let index = dir.join(VENDORED_SONG_FILE);
            if !index.is_file() {
                continue;
            }
            let Some(slug) = dir.file_name().and_then(|s| s.to_str()).map(str::to_owned) else {
                continue;
            };
            let text = std::fs::read_to_string(&index)
                .map_err(|e| ResourcesError::Io(format!("read {}: {e}", index.display())))?;

            // The song document, unless it is already on the shelf.
            let rel = resources_proto::assets::song_path(&slug);
            if shelf.root.join(&rel).exists() {
                report.kept.push(format!("song:{slug}"));
            } else {
                let doc = song::migrate_document(&text, &slug).map_err(|e| io_err(&e))?;
                shelf.put(&rel, &doc)?;
                report.migrated.push(format!("song:{slug}"));
                touched = true;
            }

            touched |= self.migrate_arrangements(&dir, &slug, &text, report)?;
        }
        if touched {
            let note = songs_dir.join(MIGRATION_NOTE);
            if !note.exists() {
                let _ = std::fs::write(&note, SONG_MIGRATION_NOTE_BODY);
            }
        }
        Ok(())
    }

    /// One song folder's arrangements → charts. Returns whether
    /// anything was written.
    fn migrate_arrangements(
        &self,
        dir: &Path,
        song_slug: &str,
        index: &str,
        report: &mut ChartMigration,
    ) -> Result<bool, ResourcesError> {
        let shelf = self.shelf(resources_proto::assets::CHARTS_KIND)?;
        let default_id = vendored_key(index, "defaultArrangement");
        let Ok(arrangements) = std::fs::read_dir(dir.join(VENDORED_ARRANGEMENTS_DIR)) else {
            return Ok(false);
        };
        // Deterministic order: the first arrangement claims the song's
        // own slug, so a rerun on a fresh disk names the same charts.
        let mut dirs: Vec<PathBuf> = arrangements
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join(VENDORED_ARRANGEMENT_FILE).is_file())
            .collect();
        dirs.sort();

        let mut wrote = false;
        for (nth, arr_dir) in dirs.iter().enumerate() {
            let note = arr_dir.join(VENDORED_ARRANGEMENT_FILE);
            let text = std::fs::read_to_string(&note)
                .map_err(|e| ResourcesError::Io(format!("read {}: {e}", note.display())))?;
            let name = vendored_key(&text, "name").unwrap_or_default();
            // The song's own slug for the first arrangement; the label
            // disambiguates the rest, through the same rule Keyflow's
            // own saves go through.
            let slug = if nth == 0 {
                song_slug.to_owned()
            } else {
                crate::asset::slug_for(&[], "", &format!("{song_slug} {name}"))
            };
            let rel = resources_proto::assets::chart_path(&slug);
            if shelf.root.join(&rel).exists() {
                report.kept.push(format!("chart:{slug}"));
                continue;
            }
            let source = std::fs::read_dir(arr_dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|e| e == "kf"))
                .and_then(|p| std::fs::read_to_string(p).ok())
                .unwrap_or_default();
            let doc = ChartDoc {
                slug: slug.clone(),
                title: vendored_key(index, "title").unwrap_or_else(|| song_slug.to_owned()),
                source,
                key: vendored_key(&text, "key").unwrap_or_default(),
                notation: chart::SOURCE.to_owned(),
                sections: Vec::new(),
                song: format!("song:{song_slug}"),
                arrangement: name,
                // The uuid pointer becomes a boolean on the chart it
                // pointed at, and the reconcile below settles the
                // invariant whatever the folder claimed.
                is_default: match (&default_id, vendored_key(&text, "id")) {
                    (Some(want), Some(have)) => *want == have,
                    // A folder with no default at all: the first
                    // arrangement is the song's main one, which is what
                    // `upsert_chart` would have decided anyway.
                    (None, _) => nth == 0,
                    _ => false,
                },
                updated_at: String::new(),
            };
            let document = chart::render_document(&doc, &slug).map_err(|e| io_err(&e))?;
            shelf.put(&rel, &document)?;
            report.migrated.push(format!("chart:{slug}"));
            wrote = true;
        }
        if wrote {
            self.reconcile_song_defaults(&format!("song:{song_slug}"), None)?;
        }
        Ok(wrote)
    }

    /// One asset group's shelf, or the refusal a host with no shelves
    /// wired in gets.
    fn shelf(&self, kind: &str) -> Result<&AssetShelf, ResourcesError> {
        self.assets.get(kind).ok_or_else(|| {
            ResourcesError::BadRequest(format!(
                "the `{kind}` asset group (ADR 0004) is not attached to this backend; \
                     charts and songs are shelf documents and there is no `std::fs` fallback"
            ))
        })
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

    /// Every chart document on the charts group, slug-sorted (that is
    /// [`walk`]'s order).
    ///
    /// Filtering on the parsed kind rather than on the extension is
    /// what keeps a person's own note on the charts shelf — a README
    /// about the folder, say — from being read as a chart with no
    /// title. The shelf is a vault directory; anybody may put a
    /// markdown file in it.
    fn charts(&self) -> Vec<LoadedResource> {
        let Ok(shelf) = self.shelf(resources_proto::assets::CHARTS_KIND) else {
            return Vec::new();
        };
        walk(&shelf.root)
            .into_iter()
            .filter(|r| r.resource.kind == crate::types::ResourceKind::Chart)
            .collect()
    }

    /// Every song document on the songs group, slug-sorted.
    ///
    /// The kind filter matters more here than for charts: the *media*
    /// tier also has a `songs` directory, and a person may well keep a
    /// note about an album on the songs shelf. Only a page that says
    /// it is a song is one.
    fn songs(&self) -> Vec<LoadedResource> {
        let Ok(shelf) = self.shelf(resources_proto::assets::SONGS_KIND) else {
            return Vec::new();
        };
        walk(&shelf.root)
            .into_iter()
            .filter(|r| r.resource.kind == crate::types::ResourceKind::Song)
            .collect()
    }

    fn chart_summary(&self, r: &LoadedResource) -> ChartSummary {
        ChartSummary {
            slug: r.resource.slug.clone(),
            title: r.resource.title.clone(),
            key: r.resource.key.clone(),
            notation: r.resource.notation.clone(),
            sections: r.resource.sections.clone(),
            song: r.resource.song.clone(),
            arrangement: r.resource.arrangement.clone(),
            is_default: r.resource.is_default,
            // Shelf-relative, because that is the only path a caller
            // can do anything with now: it is what `VaultSync::get_file`
            // and `open_collab` take, against `assets:charts`.
            // `self.rel` (resources-relative) would name a tier this
            // document is not on.
            rel_path: self
                .shelf(resources_proto::assets::CHARTS_KIND)
                .map(|s| s.rel(&r.path))
                .unwrap_or_else(|_| self.rel(&r.path)),
            updated_at: r.resource.updated_at.clone(),
        }
    }

    /// Make exactly one of a song's charts the default, and write the
    /// flag onto disk wherever it disagrees.
    ///
    /// This is the whole of the invariant
    /// [`resources_proto::ResourcesService::upsert_chart`] documents,
    /// in one place, run after every write and every delete. Doing it
    /// as a *reconciliation* rather than as a patch applied at each
    /// call site is deliberate: an upsert, a delete and a manifest
    /// somebody hand-edited in an editor all leave the same question —
    /// "which of this song's charts is the main one?" — and one answer
    /// is easier to keep right than three.
    ///
    /// The winner, in order:
    ///
    /// 1. `prefer`, when the caller has just asked for it — the write
    ///    that says "make this the main chart" wins over what was
    ///    flagged before, because it is the more recent statement of
    ///    intent.
    /// 2. Otherwise a chart already flagged, so an ordinary save of a
    ///    non-default arrangement disturbs nothing. Where more than one
    ///    is flagged (a hand-edited tree, or a manifest restored from
    ///    backup), the oldest of them wins and the rest are cleared.
    /// 3. Otherwise the **oldest remaining** chart — earliest
    ///    `updated_at`, ties broken by slug. This is the promotion rule
    ///    on delete. Oldest rather than newest because a song's first
    ///    chart is in practice the one the arrangements were cut down
    ///    from: losing a condensed live version that had been made the
    ///    main one should fall back to the original, not to whichever
    ///    alternate happened to be edited most recently. Slug breaks
    ///    ties so charts that track no timestamp still resolve
    ///    deterministically rather than by directory order.
    ///
    /// A song with no charts left is nothing to reconcile, and charts
    /// with no song are never touched: each of those is independent,
    /// and "the default one" is not a question about them.
    fn reconcile_song_defaults(
        &self,
        song: &str,
        prefer: Option<&str>,
    ) -> Result<(), ResourcesError> {
        if song.is_empty() {
            return Ok(());
        }
        let mut siblings: Vec<LoadedResource> = self
            .charts()
            .into_iter()
            .filter(|r| r.resource.song == song)
            .collect();
        if siblings.is_empty() {
            return Ok(());
        }
        // Oldest first, by (updated_at, slug) — the order rules 2 and 3
        // both read the set in.
        siblings.sort_by(|a, b| {
            (&a.resource.updated_at, &a.resource.slug)
                .cmp(&(&b.resource.updated_at, &b.resource.slug))
        });

        let winner = prefer
            .filter(|s| siblings.iter().any(|r| r.resource.slug == *s))
            .map(str::to_owned)
            .or_else(|| {
                siblings
                    .iter()
                    .find(|r| r.resource.is_default)
                    .map(|r| r.resource.slug.clone())
            })
            .unwrap_or_else(|| siblings[0].resource.slug.clone());

        for r in &siblings {
            let want = r.resource.slug == winner;
            if r.resource.is_default == want {
                continue;
            }
            let existing =
                std::fs::read_to_string(&r.path).map_err(|e| ResourcesError::Io(e.to_string()))?;
            let out = chart::set_default(&existing, want).map_err(|e| io_err(&e))?;
            // Through the vault like every other asset write: clearing
            // a sibling's flag is a write to a document somebody may
            // have open, and it has to reach their screen.
            let shelf = self.shelf(resources_proto::assets::CHARTS_KIND)?;
            shelf.put(&shelf.rel(&r.path), &out)?;
        }
        Ok(())
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

    // ── Songs: the thing charts are arrangements of ───────────────
    //
    // The chart lane again, minus the default invariant — a song has no
    // set-shaped rule to maintain, because nothing about a song is a
    // fact about its siblings. That the two lanes are otherwise the
    // same code shape is the point rather than a coincidence: ADR 0004
    // adds a shelf, not a subsystem, and a second kind on the shelf
    // should cost a `render`/`refresh` pair and nothing else.

    fn upsert_song(&self, song_doc: SongDoc) -> Result<SongUpsert, ResourcesError> {
        if song_doc.title.trim().is_empty() {
            return Err(ResourcesError::BadRequest("title is empty".into()));
        }
        let shelf = self.shelf(resources_proto::assets::SONGS_KIND)?;
        let existing = self.songs();
        let taken: Vec<String> = existing.iter().map(|r| r.resource.slug.clone()).collect();
        let slug = song::slug_for(&taken, &song_doc);
        if slug.is_empty() {
            return Err(ResourcesError::BadRequest(format!(
                "title {:?} has no sluggable characters",
                song_doc.title
            )));
        }
        let prior = existing.iter().find(|r| r.resource.slug == slug);
        let rel = prior.map_or_else(
            || resources_proto::assets::song_path(&slug),
            |r| shelf.rel(&r.path),
        );
        let abs = shelf.root.join(&rel);
        let (md, created) = match std::fs::read_to_string(&abs) {
            Ok(old) => (
                song::refresh_document(&old, &song_doc).map_err(|e| io_err(&e))?,
                false,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                song::render_document(&song_doc, &slug).map_err(|e| io_err(&e))?,
                true,
            ),
            Err(e) => return Err(ResourcesError::Io(e.to_string())),
        };
        shelf.put(&rel, &md)?;
        Ok(SongUpsert {
            slug,
            rel_path: rel,
            created,
        })
    }

    fn song(&self, slug: &str) -> Result<SongDoc, ResourcesError> {
        self.shelf(resources_proto::assets::SONGS_KIND)?;
        let found = self
            .songs()
            .into_iter()
            .find(|r| r.resource.slug == slug)
            .ok_or_else(|| ResourcesError::NotFound(slug.to_string()))?;
        Ok(SongDoc {
            slug: found.resource.slug,
            title: found.resource.title,
            writers: found.resource.writers,
            key: found.resource.key,
            tags: found.resource.tags,
            updated_at: found.resource.updated_at,
        })
    }

    fn list_songs(&self) -> Result<Vec<SongSummary>, ResourcesError> {
        let shelf = self.shelf(resources_proto::assets::SONGS_KIND)?;
        Ok(self
            .songs()
            .iter()
            .map(|r| SongSummary {
                slug: r.resource.slug.clone(),
                title: r.resource.title.clone(),
                writers: r.resource.writers.clone(),
                key: r.resource.key.clone(),
                tags: r.resource.tags.clone(),
                rel_path: shelf.rel(&r.path),
                updated_at: r.resource.updated_at.clone(),
            })
            .collect())
    }

    fn delete_song(&self, slug: &str) -> Result<bool, ResourcesError> {
        safe_segment(slug, "slug")?;
        let shelf = self.shelf(resources_proto::assets::SONGS_KIND)?;
        let Some(found) = self.songs().into_iter().find(|r| r.resource.slug == slug) else {
            return Ok(false);
        };
        // Charts naming this song are left alone — an arrangement of a
        // song that is gone reads as attached to something unresolved,
        // which is the legible state ADR 0003 chose for a dangling
        // reference and the only one a person can repair.
        shelf.delete(&shelf.rel(&found.path))?;
        Ok(true)
    }

    fn upsert_chart(&self, chart_doc: ChartDoc) -> Result<ChartUpsert, ResourcesError> {
        if chart_doc.title.trim().is_empty() {
            return Err(ResourcesError::BadRequest("title is empty".into()));
        }
        let mut chart_doc = chart_doc;
        chart_doc.song = chart::song_token(&chart_doc.song)
            .map_err(|e| ResourcesError::BadRequest(e.to_string()))?
            .unwrap_or_default();
        // An unattached chart is independent — "which is the default
        // one?" is a question about a song, and this chart names none —
        // so the flag never sticks to one.
        if chart_doc.song.is_empty() {
            chart_doc.is_default = false;
        }
        // Everything from here to the reconcile is one decision about a
        // *set* of charts, so it is taken under the lock; see
        // [`ResourcesBackend::chart_defaults`].
        let _defaults = self
            .chart_defaults
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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

        let shelf = self.shelf(resources_proto::assets::CHARTS_KIND)?;
        let prior = existing.iter().find(|r| r.resource.slug == slug);
        // `is_default: false` is *no opinion*, not "demote me". A
        // client that never tracked the flag — Keyflow saving an edit
        // to the source, the CLI without `--default` — would otherwise
        // hand the song's default to some other chart every time
        // somebody fixed a typo. So a chart that is already the default
        // of the song it is still attached to stays it, and the only
        // way to move a default is to ask for it on another chart.
        //
        // Re-attaching a chart to a *different* song does drop the
        // flag: it was the old song's main chart, and it has no claim
        // on the new song's.
        let was_default =
            prior.is_some_and(|r| r.resource.is_default && r.resource.song == chart_doc.song);
        chart_doc.is_default = chart_doc.is_default || was_default;
        // A known slug keeps the document it is already in — including
        // one a person filed somewhere else in the vault. Moving it
        // back would be Task overruling somebody's filing, and the
        // frontmatter is what says it is a chart, not the folder.
        let rel = prior.map_or_else(
            || resources_proto::assets::chart_path(&slug),
            |r| shelf.rel(&r.path),
        );
        let abs = shelf.root.join(&rel);
        let (md, created) = match std::fs::read_to_string(&abs) {
            Ok(old) => (
                chart::refresh_document(&old, &chart_doc).map_err(|e| io_err(&e))?,
                false,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                chart::render_document(&chart_doc, &slug).map_err(|e| io_err(&e))?,
                true,
            ),
            Err(e) => return Err(ResourcesError::Io(e.to_string())),
        };
        // One file, one write, through the vault — see [`AssetShelf::put`].
        shelf.put(&rel, &md)?;

        // The flag the caller asked for is a request; the invariant is
        // settled by re-reading the song's charts now that this one is
        // among them. `prefer` carries the request through: asking to
        // be the default wins, and not asking leaves whatever the song
        // already had — unless it had nothing, in which case this write
        // has just given the song its first chart and that chart is the
        // main one whatever it asked for.
        let prefer = chart_doc.is_default.then_some(slug.as_str());
        self.reconcile_song_defaults(&chart_doc.song, prefer)?;

        Ok(ChartUpsert {
            slug,
            rel_path: rel,
            created,
        })
    }

    fn chart(&self, slug: &str) -> Result<ChartDoc, ResourcesError> {
        self.shelf(resources_proto::assets::CHARTS_KIND)?;
        let found = self
            .charts()
            .into_iter()
            .find(|r| r.resource.slug == slug)
            .ok_or_else(|| ResourcesError::NotFound(slug.to_string()))?;
        // The source is a fence in the document's own body, so reading
        // it is reading the document — there is no second file to miss,
        // and no window in which a chart has a manifest but no chart.
        let document =
            std::fs::read_to_string(&found.path).map_err(|e| ResourcesError::Io(e.to_string()))?;
        let source = chart::source_of(&document);
        Ok(ChartDoc {
            slug: found.resource.slug,
            title: found.resource.title,
            source,
            key: found.resource.key,
            notation: found.resource.notation,
            sections: found.resource.sections,
            song: found.resource.song,
            arrangement: found.resource.arrangement,
            is_default: found.resource.is_default,
            updated_at: found.resource.updated_at,
        })
    }

    fn list_charts(&self, song: &str) -> Result<Vec<ChartSummary>, ResourcesError> {
        // The filter is read the same way a chart's own `song` is, so
        // `list_charts("doxology")` and `list_charts("song:doxology")`
        // are the same question.
        let want =
            chart::song_token(song).map_err(|e| ResourcesError::BadRequest(e.to_string()))?;
        Ok(self
            .charts()
            .iter()
            .filter(|r| want.as_ref().is_none_or(|s| r.resource.song == *s))
            .map(|r| self.chart_summary(r))
            .collect())
    }

    fn delete_chart(&self, slug: &str) -> Result<bool, ResourcesError> {
        safe_segment(slug, "slug")?;
        let _defaults = self
            .chart_defaults
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let shelf = self.shelf(resources_proto::assets::CHARTS_KIND)?;
        let Some(found) = self.charts().into_iter().find(|r| r.resource.slug == slug) else {
            return Ok(false);
        };
        let song = found.resource.song.clone();
        // One file, and through the vault: the `Delete` broadcast is
        // what tells an open editor the document is gone.
        shelf.delete(&shelf.rel(&found.path))?;
        // Deleting the default leaves the song with none, which the
        // invariant forbids while it still has a chart at all: the
        // reconcile promotes the oldest remaining.
        self.reconcile_song_defaults(&song, None)?;
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

    /// A backend over a tempdir with both tiers wired: the resources
    /// root every sermon/patch/sample/lighting lane writes, and the
    /// **asset groups** the chart and song lanes write (ADR 0004).
    /// Those lanes refuse outright without the second, which is the
    /// point — a misconfigured host fails loudly rather than writing
    /// charts somewhere they cannot be collaborated on.
    ///
    /// The shelves are registered here the way the server's boot loop
    /// registers them: one root per kind, under the id
    /// `org_proto::Shelf::vault_id` composes. A test that invented its
    /// own id would be testing a wiring no deployment has.
    fn backend(dir: &tempfile::TempDir) -> ResourcesBackend {
        let assets = dir.path().join("assets");
        let vault = vault::sync::Backend::with_roots(
            [
                (
                    resources_proto::assets::charts_vault_id(),
                    assets.join(resources_proto::assets::CHARTS_KIND),
                ),
                (
                    resources_proto::assets::songs_vault_id(),
                    assets.join(resources_proto::assets::SONGS_KIND),
                ),
            ]
            .into_iter()
            .collect(),
        );
        ResourcesBackend::new(dir.path().join("resources"))
            .with_assets(vault)
            .expect("attach the asset groups")
    }

    /// Where a chart lands on disk, for the assertions that check the
    /// shelf rather than the lane.
    fn chart_file(dir: &tempfile::TempDir, slug: &str) -> PathBuf {
        dir.path()
            .join("assets")
            .join(resources_proto::assets::CHARTS_KIND)
            .join(resources_proto::assets::chart_path(slug))
    }

    #[test]
    fn upsert_lays_down_three_files_and_links() {
        let dir = tempfile::tempdir().unwrap();
        let store = links::Store::open(dir.path().join("links.jsonl"));
        let be = backend(&dir).with_links(store.clone());

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
        let be = backend(&dir).with_links(store.clone());
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
        let be = backend(&dir)
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
        let be = backend(&dir)
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
        let be = backend(&dir);
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
            song: String::new(),
            arrangement: String::new(),
            is_default: false,
            updated_at: "2026-09-05T10:00:00Z".into(),
        }
    }

    /// One arrangement of a song: the chart, the label that tells it
    /// from the song's others, and what it asks to be.
    fn arrangement(title: &str, song: &str, label: &str, is_default: bool) -> ChartDoc {
        ChartDoc {
            song: song.into(),
            arrangement: label.into(),
            is_default,
            ..chart_doc(title, "| A |")
        }
    }

    /// One shelf document, the round trip, and the delete — the whole
    /// chart lane against a temp vault.
    #[test]
    fn a_chart_upsert_lays_down_one_vault_document() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend(&dir);

        let out = be
            .upsert_chart(chart_doc("Great Are You Lord", "[Verse 1]\n| A | E |\n"))
            .unwrap();
        assert_eq!(out.slug, "great-are-you-lord");
        assert_eq!(
            out.rel_path, "great-are-you-lord.md",
            "the path an app hands to VaultSync, vault-relative"
        );
        assert!(out.created);

        let md = chart_file(&dir, "great-are-you-lord");
        let text = std::fs::read_to_string(&md).expect("the document is in the vault");
        assert!(text.contains("type: asset"), "{text}");
        assert!(text.contains("asset_kind: chart"), "{text}");
        assert!(
            text.contains("[Verse 1]\n| A | E |\n"),
            "the source is in the body, verbatim: {text}"
        );
        assert!(
            !dir.path().join("resources/charts").exists(),
            "nothing is written to the tier charts left"
        );

        let back = be.chart("great-are-you-lord").unwrap();
        assert_eq!(back.source, "[Verse 1]\n| A | E |\n");
        assert_eq!(back.key, "A");
        assert_eq!(back.sections, ["verse-1", "chorus"]);

        let list = be.list_charts("").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].slug, "great-are-you-lord");
        assert_eq!(list[0].notation, "keyflow");

        assert!(be.delete_chart("great-are-you-lord").unwrap());
        assert!(!md.exists());
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
        let be = backend(&dir);
        let first = be.upsert_chart(chart_doc("Hosanna", "| A |")).unwrap();
        let second = be.upsert_chart(chart_doc("Hosanna", "| E |")).unwrap();
        assert_eq!(first.slug, "hosanna");
        assert_eq!(second.slug, "hosanna-2");
        assert!(second.created);
        assert_eq!(be.list_charts("").unwrap().len(), 2);

        let mut again = chart_doc("Hosanna (Live)", "| D |");
        again.slug = "hosanna".into();
        let update = be.upsert_chart(again).unwrap();
        assert_eq!(update.slug, "hosanna");
        assert!(!update.created, "a named slug updates rather than forks");
        assert_eq!(
            be.chart("hosanna").unwrap().source,
            "| D |\n",
            "the one byte a fence cannot preserve: a source that did not \
             end in a newline comes back with one (ADR 0004, recorded in \
             `resources_proto::assets`)"
        );
        assert_eq!(be.list_charts("").unwrap().len(), 2);
    }

    /// The contract that makes a chart safe to collaborate on: a
    /// Keyflow save rewrites the app's frontmatter and the app's fence,
    /// and leaves the paragraph a person typed beside it alone.
    #[test]
    fn chart_re_upsert_keeps_the_document_body() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend(&dir);
        be.upsert_chart(chart_doc("Hosanna", "| A |")).unwrap();
        let md = chart_file(&dir, "hosanna");
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
        assert_eq!(be.chart("hosanna").unwrap().source, "| A | E |\n");
    }

    /// ADR 0003 → ADR 0004, against a tier laid out the way production
    /// actually holds one. Three claims, and all three are about what
    /// an operator can verify by looking:
    ///
    /// - the chart is on the shelf, source and hand edits intact;
    /// - **nothing was deleted** — the originals and a breadcrumb are
    ///   still there, so the change is reversible by inspection;
    /// - running it again changes nothing, including after somebody has
    ///   edited the migrated chart. A second boot must not revert a
    ///   day's work back to the frozen snapshot.
    #[test]
    fn the_migration_copies_charts_onto_the_shelf_and_deletes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend(&dir);
        let legacy = dir.path().join("resources/charts");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("doxology.md"),
            "---\ntype: resource\nresource_kind: chart\nslug: doxology\ntitle: Doxology\ncapo: 2\n---\n# Doxology\n\n## Notes\n\n- from the hymnal\n",
        )
        .unwrap();
        std::fs::write(legacy.join("doxology.kf"), "[Verse]\n| G | C |\n").unwrap();

        let first = be.migrate_charts().unwrap();
        assert_eq!(first.migrated, ["doxology"]);
        assert!(first.kept.is_empty());

        // On the shelf, as an asset, with everything the person wrote.
        let moved = std::fs::read_to_string(chart_file(&dir, "doxology")).unwrap();
        assert!(moved.contains("type: asset"), "{moved}");
        assert!(moved.contains("capo: 2"), "foreign key survives: {moved}");
        assert!(moved.contains("- from the hymnal"), "prose survives");
        assert_eq!(be.chart("doxology").unwrap().source, "[Verse]\n| G | C |\n");

        // Nothing deleted, and a note for whoever finds the folder.
        assert!(legacy.join("doxology.md").is_file());
        assert!(legacy.join("doxology.kf").is_file());
        let note = std::fs::read_to_string(legacy.join(MIGRATION_NOTE)).unwrap();
        assert!(note.contains("copied, not moved"), "{note}");

        // A second run is a no-op — and stays one after an edit, which
        // is the case that would otherwise silently revert.
        be.upsert_chart(ChartDoc {
            slug: "doxology".into(),
            ..chart_doc("Doxology", "[Verse]\n| G | Am |\n")
        })
        .unwrap();
        let second = be.migrate_charts().unwrap();
        assert!(second.migrated.is_empty(), "{second:?}");
        assert_eq!(second.kept, ["doxology"]);
        assert_eq!(
            be.chart("doxology").unwrap().source,
            "[Verse]\n| G | Am |\n",
            "a re-run reverted an edit back to the frozen snapshot"
        );
    }

    /// The vendored song folder `task song add` used to write, turned
    /// into a song and two charts — ADR 0004's other half.
    ///
    /// The claims, and each is one of the four rows of the translation
    /// table in `SongDoc`'s docs:
    ///
    /// - the song is a document on the shelf, its uuids dropped;
    /// - each arrangement is a **chart that names its song**, not a
    ///   directory nested inside it;
    /// - `defaultArrangement`'s uuid became `is_default` on the chart
    ///   it pointed at;
    /// - the `.kf` beside each arrangement is now that chart's fence.
    ///
    /// And, as everywhere in this migration: nothing is deleted, and
    /// the media that shares the directory is not touched.
    #[test]
    fn a_vendored_song_folder_becomes_a_song_and_its_arrangements() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend(&dir);
        let folder = dir.path().join("resources/songs/opening-night");
        std::fs::create_dir_all(folder.join("arrangements/default")).unwrap();
        std::fs::create_dir_all(folder.join("arrangements/live")).unwrap();
        std::fs::write(
            folder.join("song.md"),
            "---\nid: 75e30481\ntitle: Opening Night\ntags: []\ndefaultArrangement: be760d4e\narrangements:\n- id: be760d4e\n  name: Default\n  dir: default\n  key: C Major\n---\n",
        )
        .unwrap();
        std::fs::write(
            folder.join("arrangements/default/arrangement.md"),
            "---\nid: be760d4e\nname: Default\nkey: C Major\nchartRef:\n  path: arrangements/default/opening-night.kf\n---\n",
        )
        .unwrap();
        std::fs::write(
            folder.join("arrangements/default/opening-night.kf"),
            "[Verse]\n| C | F |\n",
        )
        .unwrap();
        std::fs::write(
            folder.join("arrangements/live/arrangement.md"),
            "---\nid: aa11bb22\nname: Live\nkey: D Major\n---\n",
        )
        .unwrap();
        std::fs::write(folder.join("arrangements/live/live.kf"), "[Verse]\n| D |\n").unwrap();
        // The media half, which must survive untouched.
        std::fs::write(folder.join("manifest.json"), "{\"slug\":\"opening-night\"}").unwrap();

        let report = be.migrate_charts().unwrap();
        assert!(
            report.migrated.contains(&"song:opening-night".to_owned()),
            "{report:?}"
        );

        // The song, without its uuids.
        let song = be.song("opening-night").unwrap();
        assert_eq!(song.title, "Opening Night");
        let text = std::fs::read_to_string(
            dir.path()
                .join("assets")
                .join(resources_proto::assets::SONGS_KIND)
                .join(resources_proto::assets::song_path("opening-night")),
        )
        .unwrap();
        assert!(!text.contains("75e30481"), "{text}");
        assert!(!text.contains("defaultArrangement"), "{text}");

        // Two charts, each naming the song, with their sources folded
        // into their own fences.
        let charts = be.list_charts("song:opening-night").unwrap();
        let slugs: Vec<&str> = charts.iter().map(|c| c.slug.as_str()).collect();
        assert_eq!(
            slugs,
            ["opening-night", "opening-night-live"],
            "the first arrangement takes the song's own name"
        );
        assert_eq!(
            be.chart("opening-night").unwrap().source,
            "[Verse]\n| C | F |\n"
        );
        assert_eq!(
            be.chart("opening-night-live").unwrap().source,
            "[Verse]\n| D |\n"
        );

        // The uuid pointer became a flag on the chart it pointed at,
        // and there is exactly one.
        let defaults: Vec<&str> = charts
            .iter()
            .filter(|c| c.is_default)
            .map(|c| c.slug.as_str())
            .collect();
        assert_eq!(defaults, ["opening-night"], "{charts:?}");
        assert_eq!(
            charts
                .iter()
                .find(|c| c.slug == "opening-night-live")
                .unwrap()
                .arrangement,
            "Live",
            "the vendored `name` is the arrangement label"
        );

        // Nothing deleted, and the media the player streams is not a
        // snapshot — it never moved.
        assert!(folder.join("song.md").is_file());
        assert!(
            folder
                .join("arrangements/default/opening-night.kf")
                .is_file()
        );
        assert!(folder.join("manifest.json").is_file());
        let note = std::fs::read_to_string(dir.path().join("resources/songs").join(MIGRATION_NOTE))
            .unwrap();
        assert!(note.contains("must not be deleted"), "{note}");

        // Idempotent.
        let again = be.migrate_charts().unwrap();
        assert!(
            again.migrated.is_empty(),
            "a re-run rewrote settled files: {again:?}"
        );
    }

    /// The song lane itself, which is the chart lane minus the
    /// invariant — a round trip and a delete that leaves the
    /// arrangements alone.
    #[test]
    fn a_song_round_trips_and_deleting_it_keeps_its_charts() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend(&dir);
        let out = be
            .upsert_song(SongDoc {
                slug: String::new(),
                title: "Opening Night".into(),
                writers: vec!["A. Wright".into()],
                key: "C Major".into(),
                tags: vec!["album".into()],
                updated_at: "2026-09-09T10:00:00Z".into(),
            })
            .unwrap();
        assert_eq!(out.slug, "opening-night");
        assert_eq!(out.rel_path, "opening-night.md");
        assert!(out.created);

        let back = be.song("opening-night").unwrap();
        assert_eq!(back.title, "Opening Night");
        assert_eq!(back.writers, ["A. Wright"]);
        assert_eq!(be.list_songs().unwrap().len(), 1);

        // A chart of it, so the delete has something to leave behind.
        be.upsert_chart(ChartDoc {
            song: "song:opening-night".into(),
            ..chart_doc("Opening Night", "| C |")
        })
        .unwrap();

        assert!(be.delete_song("opening-night").unwrap());
        assert!(!be.delete_song("opening-night").unwrap());
        assert_eq!(
            be.list_charts("song:opening-night").unwrap().len(),
            1,
            "an arrangement of a song that is gone is a legible state, \
             not a reason to destroy somebody's chart"
        );
    }

    #[test]
    fn chart_without_a_title_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend(&dir);
        assert!(matches!(
            be.upsert_chart(chart_doc("   ", "| A |")),
            Err(ResourcesError::BadRequest(_))
        ));
        // A title that kebabs to nothing has no file to be, either.
        assert!(matches!(
            be.upsert_chart(chart_doc("—", "| A |")),
            Err(ResourcesError::BadRequest(_))
        ));
        assert!(be.list_charts("").unwrap().is_empty());
    }

    // ── Arrangements, and the default invariant ──────────────────

    /// Which of a song's charts is flagged, by slug.
    fn default_of(be: &ResourcesBackend, song: &str) -> Vec<String> {
        be.list_charts(song)
            .unwrap()
            .into_iter()
            .filter(|c| c.is_default)
            .map(|c| c.slug)
            .collect()
    }

    /// The three fields, end to end: two arrangements of one song, told
    /// apart by their labels, named by their labels, and filtered to
    /// the song in one call.
    #[test]
    fn two_arrangements_of_one_song_are_two_charts_that_know_it() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend(&dir);

        let original = be
            .upsert_chart(arrangement("Doxology", "song:doxology", "original", true))
            .unwrap();
        let live = be
            .upsert_chart(arrangement(
                "Doxology",
                "song:doxology",
                "condensed live",
                false,
            ))
            .unwrap();
        assert_eq!(original.slug, "doxology-original");
        assert_eq!(
            live.slug, "doxology-condensed-live",
            "the second arrangement is named for what it is, not `-2`"
        );

        let of_song = be.list_charts("song:doxology").unwrap();
        assert_eq!(of_song.len(), 2);
        assert_eq!(
            of_song
                .iter()
                .map(|c| c.arrangement.as_str())
                .collect::<Vec<_>>(),
            ["condensed live", "original"],
            "the labels come back with the list — one call renders the song"
        );
        assert_eq!(default_of(&be, "song:doxology"), ["doxology-original"]);

        // The filter reads a bare slug the same way the field does, and
        // an unrelated song sees none of this.
        assert_eq!(be.list_charts("doxology").unwrap().len(), 2);
        assert!(be.list_charts("song:hosanna").unwrap().is_empty());
    }

    /// The four halves of the invariant, in the order a person meets
    /// them: the first chart is the default whatever it asked for, a
    /// later one that asks takes it, the loser is cleared, and asking
    /// for nothing changes nothing.
    #[test]
    fn a_song_has_exactly_one_default_chart() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend(&dir);

        // 1. The first chart of a song is the default even though it
        //    asked not to be — a song with one chart and no main one is
        //    a state nothing can render.
        be.upsert_chart(arrangement("Doxology", "song:doxology", "original", false))
            .unwrap();
        assert_eq!(default_of(&be, "song:doxology"), ["doxology-original"]);

        // 2. A second arrangement that asks for nothing leaves it
        //    alone.
        be.upsert_chart(arrangement(
            "Doxology",
            "song:doxology",
            "condensed live",
            false,
        ))
        .unwrap();
        assert_eq!(default_of(&be, "song:doxology"), ["doxology-original"]);

        // 3. One that asks takes it, and the other is cleared in the
        //    same operation.
        be.upsert_chart(ChartDoc {
            slug: "doxology-condensed-live".into(),
            is_default: true,
            ..arrangement("Doxology", "song:doxology", "condensed live", true)
        })
        .unwrap();
        assert_eq!(
            default_of(&be, "song:doxology"),
            ["doxology-condensed-live"],
            "two defaults, or none, is the failure this invariant exists to stop"
        );

        // 4. A chart of a *different* song is nobody else's business.
        be.upsert_chart(arrangement("Hosanna", "song:hosanna", "", true))
            .unwrap();
        assert_eq!(default_of(&be, "song:hosanna"), ["hosanna"]);
        assert_eq!(
            default_of(&be, "song:doxology"),
            ["doxology-condensed-live"]
        );
    }

    /// An unattached chart is independent: it is never a default, and
    /// it is never anybody's arrangement. Keyflow saves one of these
    /// before the person has said what song it is, so it has to stay an
    /// ordinary state rather than a half-written one.
    #[test]
    fn a_chart_with_no_song_is_independent() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend(&dir);

        be.upsert_chart(arrangement("Sketch", "", "", true))
            .unwrap();
        be.upsert_chart(arrangement("Another Sketch", "", "", true))
            .unwrap();
        let all = be.list_charts("").unwrap();
        assert_eq!(all.len(), 2);
        assert!(
            all.iter().all(|c| !c.is_default && c.song.is_empty()),
            "an unattached chart claimed a default flag: {all:?}"
        );

        // Attaching it later is an ordinary re-save, and *then* it is
        // the song's first chart and therefore its default.
        be.upsert_chart(ChartDoc {
            slug: "sketch".into(),
            ..arrangement("Sketch", "song:doxology", "", false)
        })
        .unwrap();
        assert_eq!(default_of(&be, "song:doxology"), ["sketch"]);
    }

    /// Deleting the default promotes the oldest remaining chart of that
    /// song — earliest `updated_at`, ties broken by slug — so a song
    /// never ends up with charts and no main one.
    #[test]
    fn deleting_the_default_promotes_the_oldest_remaining() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend(&dir);

        for (label, stamp) in [
            ("original", "2026-01-01T00:00:00Z"),
            ("acoustic", "2026-02-01T00:00:00Z"),
            ("condensed live", "2026-03-01T00:00:00Z"),
        ] {
            be.upsert_chart(ChartDoc {
                updated_at: stamp.into(),
                ..arrangement(
                    "Doxology",
                    "song:doxology",
                    label,
                    label == "condensed live",
                )
            })
            .unwrap();
        }
        assert_eq!(
            default_of(&be, "song:doxology"),
            ["doxology-condensed-live"]
        );

        assert!(be.delete_chart("doxology-condensed-live").unwrap());
        assert_eq!(
            default_of(&be, "song:doxology"),
            ["doxology-original"],
            "the oldest remaining is promoted, not the most recently edited"
        );

        // Down to one, that one is it; down to none, there is nothing
        // to promote and nothing to complain about.
        assert!(be.delete_chart("doxology-original").unwrap());
        assert_eq!(default_of(&be, "song:doxology"), ["doxology-acoustic"]);
        assert!(be.delete_chart("doxology-acoustic").unwrap());
        assert!(be.list_charts("song:doxology").unwrap().is_empty());
    }

    /// A tree somebody hand-edited into two defaults (or none) is
    /// reconciled by the next write, rather than staying wrong until
    /// someone notices. The oldest of the flagged ones wins, which is
    /// the same tie-break the promotion rule uses.
    #[test]
    fn a_hand_edited_double_default_is_reconciled_by_the_next_write() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend(&dir);
        for (label, stamp) in [
            ("original", "2026-01-01T00:00:00Z"),
            ("acoustic", "2026-02-01T00:00:00Z"),
        ] {
            be.upsert_chart(ChartDoc {
                updated_at: stamp.into(),
                ..arrangement("Doxology", "song:doxology", label, false)
            })
            .unwrap();
        }
        // Somebody opened the acoustic chart in an editor and set the
        // flag by hand; now both are flagged.
        let md = chart_file(&dir, "doxology-acoustic");
        let text = std::fs::read_to_string(&md)
            .unwrap()
            .replace("is_default: false", "is_default: true");
        std::fs::write(&md, text).unwrap();
        assert_eq!(default_of(&be, "song:doxology").len(), 2);

        be.upsert_chart(ChartDoc {
            slug: "doxology-original".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            ..arrangement("Doxology", "song:doxology", "original", false)
        })
        .unwrap();
        assert_eq!(default_of(&be, "song:doxology"), ["doxology-original"]);
    }

    /// Two writers racing on one song, which is two Keyflow tabs. The
    /// backend's lock is what makes the outcome a *choice* rather than
    /// an interleaving: whichever write lands second owns the flag, and
    /// either way the song has exactly one.
    #[test]
    fn racing_writers_leave_the_song_with_exactly_one_default() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend(&dir);
        // Both arrangements exist first, so the race is purely about
        // the flag rather than about who creates the file.
        for label in ["original", "condensed live"] {
            be.upsert_chart(arrangement("Doxology", "song:doxology", label, false))
                .unwrap();
        }

        std::thread::scope(|s| {
            for (slug, label) in [
                ("doxology-original", "original"),
                ("doxology-condensed-live", "condensed live"),
            ] {
                let be = be.clone();
                s.spawn(move || {
                    for _ in 0..25 {
                        be.upsert_chart(ChartDoc {
                            slug: slug.into(),
                            ..arrangement("Doxology", "song:doxology", label, true)
                        })
                        .unwrap();
                    }
                });
            }
        });

        let flagged = default_of(&be, "song:doxology");
        assert_eq!(
            flagged.len(),
            1,
            "the race left the song with {} defaults: {flagged:?}",
            flagged.len()
        );
        assert_eq!(be.list_charts("song:doxology").unwrap().len(), 2);
    }

    /// The song field is a node reference, read the way ADR 0003 reads
    /// every reference: totally where it could be a song, refused where
    /// it names something else.
    #[test]
    fn the_song_reference_is_normalised_and_a_wrong_kind_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend(&dir);

        let bare = be
            .upsert_chart(arrangement("Doxology", "doxology", "", false))
            .unwrap();
        assert_eq!(
            be.chart(&bare.slug).unwrap().song,
            "song:doxology",
            "a bare slug is stored as this org's own song"
        );

        // A qualified reference survives verbatim, so a chart can name
        // another org's song even though writing *into* that org still
        // needs a membership row.
        let guest = be
            .upsert_chart(arrangement(
                "Hosanna",
                "guest.example/song:hosanna",
                "",
                false,
            ))
            .unwrap();
        assert_eq!(
            be.chart(&guest.slug).unwrap().song,
            "guest.example/song:hosanna"
        );
        assert_eq!(
            be.list_charts("guest.example/song:hosanna").unwrap().len(),
            1
        );

        assert!(matches!(
            be.upsert_chart(arrangement("Nope", "chart:doxology", "", false)),
            Err(ResourcesError::BadRequest(_))
        ));
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
        let be = backend(&dir);

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
        let be = backend(&dir);
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
        let be = backend(&dir);
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
        let be = backend(&dir);

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
        let be = backend(&dir);
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
        let be = backend(&dir);

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
        let be = backend(&dir);
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
        let be = backend(&dir);
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
        let be = backend(&dir);
        be.upsert_patch(patch_doc("Lead", "{}")).unwrap();
        be.upsert_sample(sample_doc("Room Kick", "{}")).unwrap();
        be.upsert_lighting(lighting_doc("Sunday Set", "{}"))
            .unwrap();
        be.upsert_chart(chart_doc("Hosanna", "| A |")).unwrap();

        assert_eq!(be.list_patches().unwrap().len(), 1);
        assert_eq!(be.list_samples().unwrap().len(), 1);
        assert_eq!(be.list_lighting().unwrap().len(), 1);
        assert_eq!(be.list_charts("").unwrap().len(), 1);
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
        let be = backend(&dir);
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
