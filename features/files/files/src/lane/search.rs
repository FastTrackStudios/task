//! `SearchService` — findability, and an honest account of its limits.
//!
//! Four rules land here, and they are not equally served. This module
//! doc says which is which up front, because a search lane that quietly
//! returns nothing for half its input is indistinguishable from one that
//! is merely empty, and the difference matters to whoever reads this
//! next.
//!
//! | Rule | State |
//! |---|---|
//! | `files.index.local` | **Satisfied by construction.** |
//! | `files.index.portable` | **Satisfied.** See the sidecar format below. |
//! | `files.index.regions` | **Satisfied** for what is extracted. |
//! | `files.index.extraction` | **Partly.** Text and technical metadata only. |
//!
//! ## `files.index.local` — satisfied because nothing external exists
//!
//! The rule's teeth are its last clause: with no external credential
//! configured, every other rule in the section still holds. That is true
//! here in the strongest possible way — there is no credential to
//! configure, no client for a third-party service, and no code path that
//! opens a socket. Extraction is `std::fs::read` plus string work on the
//! machine holding the bytes. Nothing leaves as a side effect of becoming
//! searchable because nothing leaves at all.
//!
//! Stated rather than left implicit on purpose: "we did not add an API
//! key" is only obviously a design decision if it is written down.
//!
//! ## `files.index.extraction` — what is real, and what is refused
//!
//! **Implemented.** [`Extract::Text`] over anything that is valid UTF-8:
//! notes, markdown, subtitles, session sidecars, source. [`Extract::Technical`]
//! over any file at all — name, extension, byte length, modification
//! time. Both are incremental (per path, per kind), resumable (state is
//! durable per org and a re-run continues rather than restarts), and a
//! no-op on unchanged content (the blake3 content address of the source
//! is recorded and compared before any work is done).
//!
//! **Refused, loudly.** [`Extract::Speech`] and [`Extract::Vision`] are
//! not implemented and are not faked. There is no speech recogniser and
//! no vision model anywhere in this workspace, and inventing a
//! transcript is worse than having none: a search that confidently
//! returns the wrong 12 seconds of a two-hour interview costs more trust
//! than one that admits it cannot look inside the audio. Asking for only
//! those kinds is [`FilesFault::Internal`] naming what is missing; asking
//! for them alongside a kind that works records a per-file `failed` row
//! and lets the working kind through.
//!
//! **PDF text is also refused.** The in-tree `pdf` crate is a *renderer*
//! — a wrapper over fulgur that turns HTML into PDF — and there is no
//! parser in the other direction, nor is it a dependency of this crate.
//! A `.pdf` therefore reports `not yet implemented: PDF text extraction`
//! rather than being silently skipped as "not UTF-8", so the gap reads as
//! a decision instead of an accident.
//!
//! **Failure never spreads.** No extraction failure is returned as a
//! fault for a batch that also had work it could do, nothing here writes
//! to the source file, and nothing here takes the root lock or touches
//! the version store. A file that cannot be extracted loses exactly its
//! findability, which is the rule's own wording, and
//! `tests/search_lane.rs` pins that it still browses.
//!
//! ## `files.index.portable` — the sidecar format
//!
//! A derived index is an ordinary file beside the content it describes,
//! named `<file>.<kind>.extract.txt` — so `notes/brief.md` yields
//! `notes/brief.md.text.extract.txt` in the same directory. It is UTF-8,
//! line-oriented, and readable with `cat`:
//!
//! ```text
//! task-files-extract/1
//! kind: text
//! source: notes/brief.md
//! content: b3:1f0c…
//! extracted-at: 2026-08-15T12:00:00Z
//! scheme: bytes
//!
//! @ 0 41
//! The brief opens with a line about the dog.
//!
//! @ 43 66
//! And a second paragraph.
//! ```
//!
//! The grammar, in full:
//!
//! - Line 1 is the format tag `task-files-extract/1`.
//! - Then `key: value` header lines, terminated by one blank line.
//! - Then segments. A segment opens with `@ <start> <end>` — byte offsets
//!   into **the source file**, half-open — or `@ whole` when the extract
//!   describes the file as a whole. Its text runs to the next segment
//!   header or to end of file, with the trailing blank line dropped.
//! - A text line that would itself begin with `@` is written with the `@`
//!   doubled. That is the only escape.
//!
//! Derived, and therefore disposable: deleting a sidecar loses only the
//! findability of that file until the next extraction, which regenerates
//! it byte-for-byte from the source. Nothing a user authored is ever
//! written into one — the content is a copy of bytes that already exist
//! in the source, or filesystem metadata about it.
//!
//! Sidecars live inside the root, which means they version and sync like
//! any other file. That is the price of the rule's "beside the content
//! they describe", and it is the right trade: a derived index in a hidden
//! store is not portable in any sense a person outside this application
//! would recognise.
//!
//! ## `files.index.regions` — one scheme, not a private one
//!
//! A hit carries [`Region`] — the same enum review annotations and
//! resource annotations address with, out of `files_proto::service::media`.
//! Text extraction produces `Region::Bytes { start, end }` naming the
//! block that matched, so opening a hit lands on the paragraph rather
//! than the top of the file. Technical extraction produces
//! `Region::Whole`, because "this file is 41 bytes of markdown" genuinely
//! is a statement about the whole file and pretending otherwise would be
//! a fake region.
//!
//! No second addressing type is defined here, and none should be: the
//! moment search has its own the rule is broken, whatever the two types
//! happen to contain.
//!
//! ## What this lane is not
//!
//! It is not an inverted index. `search` reads the sidecars of the rows
//! it is asked about and scans them. That is linear in extracted bytes
//! and entirely adequate for a root of notes; it is not adequate for a
//! million files, and the honest fix is a real index keyed by the same
//! content addresses this module already records, not a cleverer scan.

use facet::Facet;
use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use files_proto::error::FilesFault;
use files_proto::id::RootId;
use files_proto::model::FileRootInfo;
use files_proto::path::RootPath;
use files_proto::service::media::Region;
use files_proto::service::search::{Extract, ExtractState, Hit, Query, SearchService};

use crate::backend::FilesBackend;
use crate::error::Error;

// ── The sidecar format ─────────────────────────────────────────────

/// The format tag on line one. Versioned because the parser in
/// [`parse_sidecar`] is the only thing standing between a format change
/// and silently wrong regions.
const FORMAT_TAG: &str = "task-files-extract/1";

/// What makes a name a sidecar. Extraction skips these, and so does
/// [`SearchService::pending`]'s walk — an index of the index is not a
/// thing anybody wants, and it would never reach a fixed point.
const SIDECAR_SUFFIX: &str = ".extract.txt";

/// Above this, a file is not read into memory for text extraction.
///
/// Extraction runs on the blocking pool alongside every other org's
/// work, and a 2 GB "text" file would be a 2 GB allocation there. The
/// content address is still computed — that is streamed — so the refusal
/// is recorded against a real content hash and does not re-run on every
/// pass.
const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;

/// Extensions whose text we cannot reach, and the reason, so the failure
/// names the missing capability rather than the symptom.
///
/// Without this, a `.pdf` would fail the UTF-8 check and report
/// "unsupported: not text", which is true and useless — it reads as "PDFs
/// aren't text files" rather than "this build has no PDF parser".
const NEEDS_A_PARSER: &[(&str, &str)] = &[
    ("pdf", "PDF text extraction"),
    ("doc", "Word document text extraction"),
    ("docx", "Word document text extraction"),
    ("odt", "OpenDocument text extraction"),
    ("rtf", "RTF text extraction"),
    ("epub", "EPUB text extraction"),
];

/// The kinds this build can actually do, and the default when a caller
/// names none. Deliberately *not* every variant of [`Extract`]: a default
/// that queued speech would make every root permanently "pending".
const IMPLEMENTED: &[Extract] = &[Extract::Text, Extract::Technical];

/// The stable, lowercase tag for a kind — the wire name in a sidecar
/// header, in a sidecar filename, and in the durable state's map key.
///
/// A free function over the enum rather than a `Display` impl on it:
/// [`Extract`] lives in `files-proto` and this is one lane's file naming
/// convention, not a property of the type.
const fn tag(kind: Extract) -> &'static str {
    match kind {
        Extract::Text => "text",
        Extract::Speech => "speech",
        Extract::Vision => "vision",
        Extract::Technical => "technical",
    }
}

fn kind_of(tag: &str) -> Option<Extract> {
    match tag {
        "text" => Some(Extract::Text),
        "speech" => Some(Extract::Speech),
        "vision" => Some(Extract::Vision),
        "technical" => Some(Extract::Technical),
        _ => None,
    }
}

/// Why a kind cannot be served, or `None` if it can.
///
/// One place, so the fault text, the `failed` row and the module doc
/// cannot drift into three different accounts of the same gap.
// t[impl files.index.extraction] — failure is named, not swallowed
fn unimplemented_reason(kind: Extract) -> Option<&'static str> {
    match kind {
        Extract::Text | Extract::Technical => None,
        Extract::Speech => Some(
            "not yet implemented: speech transcription — this build has no local \
             speech recogniser, and `files.index.local` forbids reaching for a \
             third-party one to get it",
        ),
        Extract::Vision => Some(
            "not yet implemented: visual description — this build has no local \
             vision model, and `files.index.local` forbids reaching for a \
             third-party one to get it",
        ),
    }
}

/// Where a file's sidecar for one kind lives: beside it, same directory.
///
/// `None` for the root itself, which has no name to hang a sidecar off
/// and is not a file anybody extracts.
fn sidecar_for(path: &RootPath, kind: Extract) -> Option<RootPath> {
    let name = path.name()?;
    let sidecar = format!("{name}.{}{SIDECAR_SUFFIX}", tag(kind));
    match path.parent() {
        Some(parent) => parent.join(sidecar).ok(),
        None => RootPath::parse(sidecar).ok(),
    }
}

fn is_sidecar(name: &str) -> bool {
    name.ends_with(SIDECAR_SUFFIX)
}

/// One extracted block: the region of the *source* it came from, and its
/// text.
#[derive(Debug, Clone, PartialEq)]
struct Segment {
    region: Region,
    text: String,
}

/// Render a sidecar. See the module doc for the grammar.
// t[impl files.index.portable] — plain text, documented, no authored content
fn render_sidecar(
    kind: Extract,
    source: &RootPath,
    content: &str,
    at: DateTime<Utc>,
    segments: &[Segment],
) -> String {
    let scheme = match segments.first().map(|s| &s.region) {
        Some(Region::Bytes { .. }) => "bytes",
        _ => "whole",
    };
    let mut out = format!(
        "{FORMAT_TAG}\nkind: {}\nsource: {}\ncontent: {content}\nextracted-at: {}\nscheme: {scheme}\n\n",
        tag(kind),
        source.as_str(),
        at.to_rfc3339()
    );
    for segment in segments {
        match segment.region {
            Region::Bytes { start, end } => out.push_str(&format!("@ {start} {end}\n")),
            _ => out.push_str("@ whole\n"),
        }
        for line in segment.text.lines() {
            // The one escape: a body line starting with `@` would
            // otherwise be indistinguishable from the next segment's
            // header.
            if line.starts_with('@') {
                out.push('@');
            }
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// Read a sidecar back. Anything malformed yields no segments rather
/// than an error: a corrupt derived file costs its own file's
/// findability, exactly as a missing one does, and must never fail a
/// query over the other files that parsed.
fn parse_sidecar(text: &str) -> Vec<Segment> {
    let Some(body) = text
        .strip_prefix(FORMAT_TAG)
        .and_then(|rest| rest.split_once("\n\n"))
        .map(|(_headers, body)| body)
    else {
        return Vec::new();
    };

    let mut segments: Vec<Segment> = Vec::new();
    let mut current: Option<(Region, String)> = None;
    for line in body.lines() {
        if let Some(region) = segment_header(line) {
            if let Some((region, text)) = current.take() {
                segments.push(finish(region, text));
            }
            current = Some((region, String::new()));
            continue;
        }
        if let Some((_, text)) = current.as_mut() {
            // Un-double the escape, keeping the `@` that was escaped.
            match line.strip_prefix("@@") {
                Some(rest) => {
                    text.push('@');
                    text.push_str(rest);
                }
                None => text.push_str(line),
            }
            text.push('\n');
        }
    }
    if let Some((region, text)) = current {
        segments.push(finish(region, text));
    }
    segments
}

fn finish(region: Region, text: String) -> Segment {
    Segment {
        region,
        text: text.trim_end_matches('\n').to_string(),
    }
}

/// `@ 0 41` or `@ whole`, and nothing else — a body line that merely
/// starts with `@` is not a header.
fn segment_header(line: &str) -> Option<Region> {
    let rest = line.strip_prefix("@ ")?;
    if rest == "whole" {
        return Some(Region::Whole);
    }
    let (start, end) = rest.split_once(' ')?;
    Some(Region::Bytes {
        start: start.parse().ok()?,
        end: end.parse().ok()?,
    })
}

// ── Extraction ─────────────────────────────────────────────────────

/// Split text into blank-line-separated blocks, carrying each block's
/// byte offsets in the source.
///
/// A block rather than a line because a block is what a person means by
/// "where in the document": a hit on a single line of a wrapped paragraph
/// opens at a fragment, and a hit on the whole file is the thing
/// `files.index.regions` exists to forbid.
// t[impl files.index.regions] — offsets into the source, not into a copy
fn blocks(text: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let mut start: Option<usize> = None;
    let mut end = 0usize;

    let flush = |start: &mut Option<usize>, end: usize, out: &mut Vec<Segment>| {
        if let Some(s) = start.take() {
            out.push(Segment {
                region: Region::Bytes {
                    start: s as u64,
                    end: end as u64,
                },
                text: text[s..end].to_string(),
            });
        }
    };

    for line in text.split_inclusive('\n') {
        if line.trim().is_empty() {
            flush(&mut start, end, &mut out);
        } else {
            if start.is_none() {
                start = Some(offset);
            }
            end = offset + line.trim_end_matches(['\n', '\r']).len();
        }
        offset += line.len();
    }
    flush(&mut start, end, &mut out);
    out
}

/// The content address of a file, streamed.
///
/// blake3 because it is already this feature's content-addressing
/// function — `files_store::chunk` re-exports the exact crate so a second
/// hash cannot drift from the store's. Streamed in fixed buffers so the
/// no-op check is affordable on a file far too large to extract.
// t[impl files.index.extraction] — the address a no-op is decided on
fn content_address(disk: &Path) -> Result<String, std::io::Error> {
    use std::io::Read;

    let mut file = std::fs::File::open(disk)?;
    let mut hasher = files_store::chunk::blake3::Hasher::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("b3:{}", hasher.finalize().to_hex()))
}

/// Technical metadata: what the filesystem knows, written as headers a
/// person can read.
///
/// **This is less than the rule wants.** `files.index.extraction` names
/// camera, lens, codec and timecode. Reaching those needs `ffprobe`, and
/// the only ffprobe in this workspace is behind
/// `files_transcode::Transcoder`, whose sole classification method
/// answers `MediaClass` — video / audio / other — and is not reachable
/// from this module in any case (the backend's transcoder field has no
/// accessor). Extending this to real media metadata is a new method on
/// that trait, not more code here.
fn technical(disk: &Path, path: &RootPath) -> Result<Vec<Segment>, std::io::Error> {
    let meta = std::fs::metadata(disk)?;
    let mut text = String::new();
    if let Some(name) = path.name() {
        text.push_str(&format!("name: {name}\n"));
    }
    if let Some(ext) = Path::new(path.as_str())
        .extension()
        .and_then(|e| e.to_str())
    {
        text.push_str(&format!("extension: {}\n", ext.to_lowercase()));
    }
    text.push_str(&format!("bytes: {}\n", meta.len()));
    if let Ok(modified) = meta.modified() {
        text.push_str(&format!(
            "modified: {}\n",
            DateTime::<Utc>::from(modified).to_rfc3339()
        ));
    }
    Ok(vec![Segment {
        // A statement about the file as a whole really is `Whole`.
        // Manufacturing a byte range here would be a fake region, which
        // is worse than an honest coarse one.
        region: Region::Whole,
        text: text.trim_end().to_string(),
    }])
}

/// Text, or the reason there is none.
fn text_segments(disk: &Path, path: &RootPath, len: u64) -> Result<Vec<Segment>, String> {
    let ext = Path::new(path.as_str())
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    if let Some((_, what)) = NEEDS_A_PARSER.iter().find(|(e, _)| *e == ext) {
        return Err(format!("not yet implemented: {what}"));
    }
    if len > MAX_TEXT_BYTES {
        return Err(format!(
            "too large to extract in memory: {len} bytes exceeds the {MAX_TEXT_BYTES}-byte cap"
        ));
    }
    let bytes = std::fs::read(disk).map_err(|e| format!("unreadable: {e}"))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "unsupported for text extraction: content is not UTF-8".to_string())?;
    Ok(blocks(text))
}

// ── Durable state ──────────────────────────────────────────────────

/// What has been extracted for this org, and from which content.
///
/// Per org via [`crate::durable::Scoped`], keyed by the backend's data
/// dir. A module-level static would be shared by every org on the server,
/// which is the cross-org leak the durable module exists to make
/// impossible — and `pending` and `search` both enumerate without a root
/// to scope them, so this lane is precisely the shape that leaked.
///
/// Durable so that extraction is *resumable* in the sense the rule means:
/// a server restarted halfway through a root does not re-extract what it
/// already did, because the content addresses survived.
#[derive(Debug, Default, Clone, Facet)]
#[repr(C)]
struct Extracted {
    rows: BTreeMap<(RootId, RootPath, &'static str), Row>,
}

/// One file, one kind.
#[derive(Debug, Clone, Facet)]
#[repr(C)]
struct Row {
    root: RootId,
    path: RootPath,
    kind: String,
    /// The content address the sidecar was made from. A re-run whose
    /// source still hashes to this does nothing.
    content: Option<String>,
    sidecar: Option<RootPath>,
    failed: Option<String>,
    done: bool,
    updated_at: DateTime<Utc>,
}

/// The on-disk shape: a list, because JSON has no composite map key.
/// Same trade as `lane::organise` — the runtime type stays the one every
/// lookup wants and the conversion lives here.
#[derive(Default, Facet)]
#[repr(C)]
struct Wire {
    rows: Vec<Row>,
}

impl From<Wire> for Extracted {
    fn from(w: Wire) -> Self {
        Self {
            rows: w
                .rows
                .into_iter()
                .filter_map(|r| {
                    let kind = kind_of(&r.kind)?;
                    Some(((r.root, r.path.clone(), tag(kind)), r))
                })
                .collect(),
        }
    }
}

impl From<Extracted> for Wire {
    fn from(e: Extracted) -> Self {
        Self {
            rows: e.rows.into_values().collect(),
        }
    }
}

static EXTRACTED: crate::durable::Scoped<Extracted> = crate::durable::Scoped::new("search");

impl Row {
    fn state(&self) -> ExtractState {
        ExtractState {
            root_id: self.root,
            path: self.path.clone(),
            kind: kind_of(&self.kind).unwrap_or(Extract::Text),
            done: self.done,
            failed: self.failed.clone(),
            sidecar: self.sidecar.clone(),
            updated_at: self.updated_at,
        }
    }
}

/// A row for a file nothing has touched yet — outstanding, not failed.
fn untouched(root: RootId, path: RootPath, kind: Extract) -> ExtractState {
    ExtractState {
        root_id: root,
        path,
        kind,
        done: false,
        failed: None,
        sidecar: None,
        // No extraction has happened, so there is no moment to report;
        // the epoch says "never" without inventing a timestamp.
        updated_at: DateTime::UNIX_EPOCH,
    }
}

// ── The work ───────────────────────────────────────────────────────

/// Extract one kind from one file, writing its sidecar.
///
/// Infallible by design: every failure becomes a `failed` row rather than
/// an `Err`. That is `files.index.extraction`'s last clause made
/// structural — there is no return path through which one file's problem
/// can reach the caller as a batch failure, so it cannot block anything.
// t[impl files.index.extraction] — incremental, resumable, no-op on
// unchanged content, and a failure costs one file its findability
fn extract_one(
    backend: &FilesBackend,
    root: &FileRootInfo,
    path: &RootPath,
    kind: Extract,
    now: DateTime<Utc>,
) -> ExtractState {
    let fail = |reason: String| ExtractState {
        root_id: RootId::new(root.id),
        path: path.clone(),
        kind,
        done: false,
        failed: Some(reason),
        sidecar: None,
        updated_at: now,
    };

    if let Some(reason) = unimplemented_reason(kind) {
        return record(backend, fail(reason.to_string()), None);
    }
    let Some(sidecar) = sidecar_for(path, kind) else {
        return fail("the root itself is not an extractable file".into());
    };
    if path.name().is_some_and(is_sidecar) {
        return fail("a derived index is not itself extracted".into());
    }

    let Ok((disk, _)) = backend.resolve_root_file(root, path.as_str()) else {
        return fail("path escapes the root".into());
    };
    let meta = match std::fs::metadata(&disk) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return fail("not a file".into()),
        Err(err) => return fail(format!("unreadable: {err}")),
    };
    let content = match content_address(&disk) {
        Ok(content) => content,
        Err(err) => return fail(format!("unreadable: {err}")),
    };

    // The no-op. Both halves matter: the content must be the one we
    // extracted, *and* the sidecar must still be there — a user who
    // deleted a derived file is entitled to get it back, and a hash match
    // alone would leave them permanently unfindable.
    let sidecar_disk = backend
        .resolve_root_file(root, sidecar.as_str())
        .map(|(disk, _)| disk)
        .ok();
    let unchanged = EXTRACTED.read(backend, |state| {
        state
            .rows
            .get(&(RootId::new(root.id), path.clone(), tag(kind)))
            .filter(|row| row.done && row.content.as_deref() == Some(content.as_str()))
            .map(Row::state)
    });
    if let Some(existing) = unchanged {
        if sidecar_disk.as_deref().is_some_and(Path::exists) {
            return existing;
        }
    }

    let segments = match kind {
        Extract::Text => match text_segments(&disk, path, meta.len()) {
            Ok(segments) => segments,
            Err(reason) => return record(backend, fail(reason), Some(content)),
        },
        Extract::Technical => match technical(&disk, path) {
            Ok(segments) => segments,
            Err(err) => return record(backend, fail(format!("unreadable: {err}")), Some(content)),
        },
        // Handled above; matching exhaustively rather than with a
        // wildcard so a fifth kind is a compile error here.
        Extract::Speech | Extract::Vision => unreachable!("refused above"),
    };

    let Some(sidecar_disk) = sidecar_disk else {
        return record(
            backend,
            fail("sidecar path escapes the root".into()),
            Some(content),
        );
    };
    let rendered = render_sidecar(kind, path, &content, now, &segments);
    if let Err(err) = write_atomically(&sidecar_disk, rendered.as_bytes()) {
        return record(
            backend,
            fail(format!("sidecar not written: {err}")),
            Some(content),
        );
    }

    record(
        backend,
        ExtractState {
            root_id: RootId::new(root.id),
            path: path.clone(),
            kind,
            done: true,
            failed: None,
            sidecar: Some(sidecar),
            updated_at: now,
        },
        Some(content),
    )
}

/// A sidecar is replaced, never truncated-then-filled.
///
/// A crash mid-write would otherwise leave a half-written derived file
/// whose header parses and whose last segment is a lie — and unlike a
/// missing sidecar, a truncated one is not obviously wrong to the reader.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("txt.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Persist a state as a row and hand it back unchanged.
fn record(backend: &FilesBackend, state: ExtractState, content: Option<String>) -> ExtractState {
    EXTRACTED.write(backend, |s| {
        s.rows.insert(
            (state.root_id, state.path.clone(), tag(state.kind)),
            Row {
                root: state.root_id,
                path: state.path.clone(),
                kind: tag(state.kind).to_string(),
                content,
                sidecar: state.sidecar.clone(),
                failed: state.failed.clone(),
                done: state.done,
                updated_at: state.updated_at,
            },
        );
    });
    state
}

/// Every extractable file in a root, relative paths, depth-first.
///
/// Through `browse_inner` rather than a raw `read_dir` walk so the root's
/// own internals (`STORE_DIR`, the marker) are hidden by the same code
/// that hides them everywhere else, and so pointer stubs are visible as
/// entries — a stub is a real file that is simply not resident, and it is
/// outstanding rather than absent.
fn walk(backend: &FilesBackend, root_id: RootId, at: &RootPath) -> Result<Vec<RootPath>, Error> {
    let mut out = Vec::new();
    let entries = backend.browse_inner(root_id.get(), at.as_str().to_string())?;
    for entry in entries {
        let Ok(child) = at.join(&entry.name) else {
            continue;
        };
        if entry.is_dir {
            out.extend(walk(backend, root_id, &child)?);
        } else if !is_sidecar(&entry.name) {
            out.push(child);
        }
    }
    Ok(out)
}

// ── Query ──────────────────────────────────────────────────────────

/// The terms a query is made of: lowercase, whitespace-separated.
fn terms(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(str::to_lowercase)
        .filter(|t| !t.is_empty())
        .collect()
}

/// Score a segment against the terms, or `None` if it does not match.
///
/// Every term must appear — more terms narrow a result rather than
/// widening it, the same choice `lane::organise::tagged` makes for tags,
/// and for the same reason: a multi-word query that returns *more* is not
/// what anybody typed it for.
///
/// The score is match density rather than raw count so that a short note
/// mentioning the term once outranks a long log that mentions it once.
fn score(text: &str, terms: &[String]) -> Option<f32> {
    let haystack = text.to_lowercase();
    let mut hits = 0usize;
    for term in terms {
        let count = haystack.matches(term.as_str()).count();
        if count == 0 {
            return None;
        }
        hits += count;
    }
    let length = haystack.chars().count().max(1) as f32;
    Some(
        (hits as f32 / length.sqrt())
            .min(1.0)
            .max(f32::MIN_POSITIVE),
    )
}

/// A readable window around the first match.
fn excerpt(text: &str, terms: &[String]) -> String {
    const WIDTH: usize = 240;
    let lower = text.to_lowercase();
    let at = terms
        .iter()
        .filter_map(|t| lower.find(t.as_str()))
        .min()
        .unwrap_or(0);
    // Char boundaries, not byte arithmetic: slicing a UTF-8 string at a
    // computed byte offset panics on the first accented word.
    let chars: Vec<char> = text.chars().collect();
    let at = text[..at].chars().count();
    let start = at.saturating_sub(WIDTH / 4);
    let end = (start + WIDTH).min(chars.len());
    let mut out: String = chars[start..end].iter().collect();
    if start > 0 {
        out.insert(0, '…');
    }
    if end < chars.len() {
        out.push('…');
    }
    out
}

// ── The lane ───────────────────────────────────────────────────────

impl SearchService for FilesBackend {
    // t[impl files.index.regions] — every hit carries the shared `Region`,
    // and a text hit carries the block's byte range rather than `Whole`
    async fn search(&self, query: Query) -> Result<Vec<Hit>, FilesFault> {
        if let Some(root_id) = query.root_id {
            crate::lane::root_or_fault(self, root_id)?;
        }
        let under = query.under.clone().map(|u| u.validate()).transpose()?;
        let terms = terms(&query.text);
        if terms.is_empty() {
            // An empty query is not "everything": there is no ranking
            // that makes sense for it, and returning the whole index is
            // how a search box becomes a denial of service.
            return Ok(Vec::new());
        }
        let backend = self.clone();

        crate::lane::blocking(move || {
            let rows: Vec<Row> = EXTRACTED.read(&backend, |s| {
                s.rows
                    .values()
                    .filter(|r| r.done && r.sidecar.is_some())
                    .filter(|r| query.root_id.is_none_or(|id| id == r.root))
                    .filter(|r| {
                        under
                            .as_ref()
                            .is_none_or(|u| u.is_root() || r.path.is_within(u))
                    })
                    .filter(|r| {
                        query.kinds.is_empty()
                            || kind_of(&r.kind).is_some_and(|k| query.kinds.contains(&k))
                    })
                    .cloned()
                    .collect()
            });

            let mut hits = Vec::new();
            for row in rows {
                let Ok(root) = backend.get_root_info(row.root.get()) else {
                    // A row for a root that has since been forgotten is
                    // stale, not an error.
                    continue;
                };
                let Some(sidecar) = row.sidecar.as_ref() else {
                    continue;
                };
                let Ok((disk, _)) = backend.resolve_root_file(&root, sidecar.as_str()) else {
                    continue;
                };
                // A deleted sidecar loses findability and nothing else —
                // which is exactly what `files.index.portable` promises.
                let Ok(text) = std::fs::read_to_string(&disk) else {
                    continue;
                };
                let kind = kind_of(&row.kind).unwrap_or(Extract::Text);
                for segment in parse_sidecar(&text) {
                    if let Some(score) = score(&segment.text, &terms) {
                        hits.push(Hit {
                            root_id: row.root,
                            path: row.path.clone(),
                            region: segment.region.clone(),
                            kind,
                            excerpt: excerpt(&segment.text, &terms),
                            score,
                        });
                    }
                }
            }

            hits.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.path.cmp(&b.path))
            });
            if let Some(limit) = query.limit {
                hits.truncate(limit as usize);
            }
            Ok(hits)
        })
        .await
    }

    // t[impl files.index.extraction] — what is known about one file
    async fn extract_state(
        &self,
        root_id: RootId,
        path: RootPath,
    ) -> Result<Vec<ExtractState>, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let path = path.validate()?;
        Ok(EXTRACTED.read(self, |s| {
            IMPLEMENTED
                .iter()
                .map(|kind| {
                    s.rows
                        .get(&(root_id, path.clone(), tag(*kind)))
                        .map_or_else(|| untouched(root_id, path.clone(), *kind), Row::state)
                })
                .collect()
        }))
    }

    /// The honest answer to "is it all searchable yet", which means
    /// walking the tree rather than reading the state file: a file nobody
    /// has attempted is outstanding, and a state file that has never
    /// heard of it would report an empty queue and a complete index.
    ///
    /// Rows that *failed* are reported too. They are not going to be
    /// retried on their own and they are not searchable, so omitting them
    /// would make `pending` say "done" about a root half of which cannot
    /// be found.
    ///
    /// **Known gap:** a file whose content changed since it was extracted
    /// is reported as done. Detecting it needs the content address of
    /// every file in the root, and hashing the tree on every `pending`
    /// call is not a cost this method can carry. The re-extraction path
    /// notices immediately (that is what the address is for); the queue
    /// does not.
    // t[impl files.index.extraction] — incremental means there is a
    // remainder, and it is nameable
    async fn pending(&self, root_id: RootId) -> Result<Vec<ExtractState>, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let backend = self.clone();
        crate::lane::blocking(move || {
            let paths = walk(&backend, root_id, &RootPath::root())?;
            Ok(EXTRACTED.read(&backend, |s| {
                let mut out = Vec::new();
                for path in paths {
                    for kind in IMPLEMENTED {
                        match s.rows.get(&(root_id, path.clone(), tag(*kind))) {
                            Some(row) if row.done => {}
                            Some(row) => out.push(row.state()),
                            None => out.push(untouched(root_id, path.clone(), *kind)),
                        }
                    }
                }
                out
            }))
        })
        .await
    }

    /// Extract now, ahead of whatever queue exists.
    ///
    /// Returns a fault only when it could do *nothing at all* — every
    /// requested kind unimplemented. A batch with one workable kind in it
    /// succeeds, and the unworkable kinds come back as `failed` rows,
    /// because a caller asking for text and speech together should get
    /// its text.
    // t[impl files.index.extraction] — a request for the impossible says so
    // t[impl files.index.local] — no credential, no socket, no third party
    async fn extract(
        &self,
        root_id: RootId,
        paths: Vec<RootPath>,
        kinds: Vec<Extract>,
    ) -> Result<Vec<ExtractState>, FilesFault> {
        let root = crate::lane::root_or_fault(self, root_id)?;
        let paths = paths
            .iter()
            .map(RootPath::validate)
            .collect::<Result<Vec<_>, _>>()?;
        let kinds = if kinds.is_empty() {
            IMPLEMENTED.to_vec()
        } else {
            kinds
        };

        if let Some(reason) = kinds
            .iter()
            .all(|k| unimplemented_reason(*k).is_some())
            .then(|| {
                kinds
                    .iter()
                    .filter_map(|k| unimplemented_reason(*k))
                    .collect::<Vec<_>>()
                    .join("; ")
            })
        {
            return Err(FilesFault::Internal(reason));
        }

        let backend = self.clone();
        crate::lane::blocking(move || {
            let now = Utc::now();
            let paths = if paths.is_empty() {
                walk(&backend, root_id, &RootPath::root())?
            } else {
                paths
            };
            let mut out = Vec::with_capacity(paths.len() * kinds.len());
            for path in &paths {
                for kind in &kinds {
                    out.push(extract_one(&backend, &root, path, *kind, now));
                }
            }
            Ok(out)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // t[verify files.index.regions]
    #[test]
    fn a_block_carries_its_offsets_in_the_source() {
        let text = "first para\nsecond line\n\nlater para\n";
        let segments = blocks(text);
        assert_eq!(segments.len(), 2);
        let Region::Bytes { start, end } = segments[1].region else {
            panic!("a text block is a byte range, never Whole");
        };
        assert_eq!(&text[start as usize..end as usize], "later para");
    }

    // t[verify files.index.portable]
    #[test]
    fn a_sidecar_round_trips_through_its_own_grammar() {
        let source = RootPath::parse("notes/brief.md").unwrap();
        let segments = blocks("a dog\n\n@ not a header\nstill text\n");
        let rendered = render_sidecar(
            Extract::Text,
            &source,
            "b3:cafe",
            DateTime::UNIX_EPOCH,
            &segments,
        );
        assert!(
            rendered.starts_with(FORMAT_TAG),
            "line one names the format"
        );
        assert!(
            rendered.contains("@@ not a header"),
            "a body line starting with `@` is escaped by doubling"
        );
        assert_eq!(parse_sidecar(&rendered), segments);
    }

    // t[verify files.index.portable]
    #[test]
    fn a_corrupt_sidecar_reads_as_empty_rather_than_panicking() {
        assert!(parse_sidecar("not a sidecar at all").is_empty());
        assert!(parse_sidecar("").is_empty());
    }

    // t[verify files.index.extraction]
    #[test]
    fn every_term_must_match() {
        let terms = terms("Dog Brief");
        assert!(score("the dog in the brief", &terms).is_some());
        assert!(
            score("the dog", &terms).is_none(),
            "more terms narrow a result, they do not widen it"
        );
    }

    #[test]
    fn an_excerpt_never_splits_a_character() {
        let terms = terms("café");
        let text = "ééééé café ééééé";
        assert!(excerpt(text, &terms).contains("café"));
    }

    // t[verify files.index.portable]
    #[test]
    fn a_sidecar_lives_beside_what_it_describes() {
        let path = RootPath::parse("notes/brief.md").unwrap();
        assert_eq!(
            sidecar_for(&path, Extract::Text).unwrap().as_str(),
            "notes/brief.md.text.extract.txt"
        );
        assert!(
            sidecar_for(&RootPath::root(), Extract::Text).is_none(),
            "the root is not a file"
        );
    }
}

impl crate::durable::Durable for Extracted {
    type Wire = Wire;

    fn to_wire(&self) -> Wire {
        Wire::from(self.clone())
    }

    fn from_wire(wire: Wire) -> Self {
        Self::from(wire)
    }
}
