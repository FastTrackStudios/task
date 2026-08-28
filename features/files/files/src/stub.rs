//! The on-disk **pointer stub** format (issue #263, glossary "Pointer
//! stub"): a small placeholder file standing in for non-resident content
//! inside a live tree — a dehydrated file.
//!
//! The stub file IS the authority on dehydration state — there is no
//! sidecar index to drift from it (ADR 0001's single-authority
//! doctrine). Raw NFS reads a stub as a stub: the format is one honest
//! plain-text line saying what it is, then a JSON body carrying what
//! hydration needs — the exact `FileId` the content had when it was
//! dehydrated, the logical size listings must keep reporting, and the
//! executable bit to restore.
//!
//! Detection is cheap by construction: a stub is never larger than
//! [`MAX_LEN`], so a listing distinguishes resident from stub with one
//! stat plus — only for files small enough to qualify — one short read
//! of the magic line. No media file is ever opened to answer.

use std::io::Read as _;
use std::path::Path;

use facet::Facet;
use jj_lib::backend::FileId;
use jj_lib::object_id::ObjectId as _;

use crate::error::{Error, Result};

/// First line of every stub, newline included. The leading `#` keeps a
/// DAW or shell that blindly opens one from binary-garbage territory —
/// a human (or NFS client) sees a one-line explanation, not noise.
pub const MAGIC: &str = "#fts-stub v1\n";

/// A stub file never exceeds this many bytes — the detection bound.
/// Generous: magic + JSON with a 32-byte hex id is well under 256.
pub const MAX_LEN: u64 = 4096;

/// What a stub records about the content it stands in for.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct Stub {
    /// Hex `FileId` of the dehydrated content — the identity hydration
    /// restores and verifies (media roots: the CAS chunk-manifest hash).
    pub file_id: String,
    /// Logical size of the dehydrated content in bytes, preserved so
    /// listings keep reporting the real size while nothing is resident.
    pub size: u64,
    /// The executable bit the content carried, restored on hydration.
    pub executable: bool,
}

impl Stub {
    #[must_use]
    pub fn new(file_id: &FileId, size: u64, executable: bool) -> Self {
        Self {
            file_id: file_id.hex(),
            size,
            executable,
        }
    }

    /// The recorded id as a jj [`FileId`].
    pub fn file_id(&self) -> Result<FileId> {
        let bytes = (0..self.file_id.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(self.file_id.get(i..i + 2).unwrap_or_default(), 16))
            .collect::<std::result::Result<Vec<u8>, _>>()
            .map_err(|_| {
                Error::BadRequest(format!("stub carries a malformed id: {}", self.file_id))
            })?;
        Ok(FileId::new(bytes))
    }

    /// Serialize to the on-disk representation.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = MAGIC.as_bytes().to_vec();
        // Infallible: the struct is three plain scalars.
        out.extend(
            facet_json::to_string(self)
                .expect("stub serializes")
                .into_bytes(),
        );
        out.push(b'\n');
        out
    }

    /// Parse the on-disk representation. `None` when the bytes are not
    /// a stub at all (no magic); `Err` when the magic is present but
    /// the body doesn't parse — that file claims to be a stub and
    /// can't be trusted as anything else, so callers must surface it
    /// rather than treat it as content.
    pub fn from_bytes(bytes: &[u8]) -> Result<Option<Self>> {
        let Some(body) = bytes.strip_prefix(MAGIC.as_bytes()) else {
            return Ok(None);
        };
        facet_json::from_slice(body)
            .map(Some)
            .map_err(|e| Error::BadRequest(format!("malformed stub body: {e}")))
    }
}

/// Read `path` as a stub if it is one. `Ok(None)` for ordinary content
/// (including anything larger than [`MAX_LEN`], which is disqualified
/// by its stat alone — no read). Errors propagate: an unreadable file
/// must never silently pass as "not a stub" (the fail-open shape PR
/// #287's policy review finding warned about).
pub fn read(path: &Path) -> Result<Option<Stub>> {
    let len = std::fs::metadata(path)?.len();
    if len > MAX_LEN || len < MAGIC.len() as u64 {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(len as usize);
    std::fs::File::open(path)?.read_to_end(&mut bytes)?;
    Stub::from_bytes(&bytes)
}

/// Cheap pre-filter for walkers that already hold a stat: only a file
/// whose length could be a stub warrants the header read.
#[must_use]
pub fn candidate_len(len: u64) -> bool {
    len <= MAX_LEN && len >= MAGIC.len() as u64
}

/// The **lenient** twin of [`read`], for enumeration paths (listings,
/// checkpoint scans) where one odd small file must never take down the
/// whole operation (PR #289 review): any error — unreadable, vanished
/// mid-listing, magic with a garbage body — logs and answers "not a
/// stub", so the file is handled as the ordinary content its bytes are.
/// Surfaces that *act* on stub-ness (dehydrate/hydrate, the WebDAV
/// serve path) keep the strict [`read`] and fail closed instead: there,
/// mistaking a broken stub for content would serve or destroy
/// placeholder bytes.
#[must_use]
pub fn probe(path: &Path) -> Option<Stub> {
    match read(path) {
        Ok(found) => found,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                %err,
                "unreadable or malformed stub-shaped file treated as ordinary content",
            );
            None
        }
    }
}

/// Atomically write `stub` over `path` (tmp + rename in the same
/// directory, fsynced — a crash mid-dehydrate must leave either the
/// original content or a whole stub, never a torn one).
pub fn write(path: &Path, stub: &Stub) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::BadRequest(format!("{}: no parent directory", path.display())))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::io::Write::write_all(&mut tmp, &stub.to_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| Error::Io(e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let stub = Stub::new(&FileId::new(vec![0xab; 32]), 1_234_567, true);
        let parsed = Stub::from_bytes(&stub.to_bytes()).unwrap().unwrap();
        assert_eq!(parsed, stub);
        assert_eq!(parsed.file_id().unwrap(), FileId::new(vec![0xab; 32]));
    }

    #[test]
    fn ordinary_content_is_not_a_stub() {
        assert_eq!(Stub::from_bytes(b"RIFF....WAVE").unwrap(), None);
    }

    #[test]
    fn magic_with_garbage_body_errors() {
        let mut bytes = MAGIC.as_bytes().to_vec();
        bytes.extend(b"not json");
        assert!(Stub::from_bytes(&bytes).is_err());
    }
}
