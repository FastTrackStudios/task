//! Chunk manifests: the Files-owned record of "which chunks, in which
//! order, make up this file". Kept outside the iroh-blobs store (as plain
//! files under a store's `manifests/` directory) so the store is
//! rebuildable per ADR 0001 — the manifest is the only thing that turns a
//! bag of content-addressed chunks back into a file.

use crate::error::{Error, Result};

/// One chunk's identity within a manifest: its content hash and its
/// (uncompressed) byte length. Length is recorded here — not derived from
/// the blob store — so a manifest is self-describing even if the backing
/// blob is briefly absent (e.g. mid-hydration).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkRef {
    pub hash: blake3::Hash,
    pub len: u64,
}

/// The ordered list of chunks that make up one file's content. A
/// manifest's own identity — its [`FileId`] — is the blake3 hash of its
/// canonical encoding, so two files with byte-identical content always
/// produce the same manifest and the same `FileId`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Manifest {
    pub chunks: Vec<ChunkRef>,
}

/// A file's content-address: the hash of its [`Manifest`]'s canonical
/// encoding. Two saves with identical bytes — and therefore an identical
/// chunk sequence — resolve to the same `FileId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(blake3::Hash);

// `blake3::Hash` has no `Ord`/`PartialOrd` impl of its own; order by raw
// bytes so `FileId` can key a `BTreeSet`/`BTreeMap` (e.g. `ChunkStore::gc`'s
// `protected` set).
impl PartialOrd for FileId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FileId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.as_bytes().cmp(other.0.as_bytes())
    }
}

const MAGIC: &[u8; 4] = b"FTSM";
const VERSION: u8 = 1;
/// magic(4) + version(1) + chunk count(4)
const HEADER_LEN: usize = 4 + 1 + 4;
/// hash(32) + len(8) per chunk entry
const ENTRY_LEN: usize = 32 + 8;

impl Manifest {
    pub fn new(chunks: Vec<ChunkRef>) -> Self {
        Self { chunks }
    }

    /// Total length of the file this manifest describes, in bytes.
    pub fn total_len(&self) -> u64 {
        self.chunks.iter().map(|c| c.len).sum()
    }

    /// This manifest's canonical, deterministic on-disk encoding: a fixed
    /// header followed by one 40-byte `(hash, len)` entry per chunk, in
    /// chunk order. Byte-identical manifests always encode to the same
    /// bytes, which is what makes [`Manifest::file_id`] a stable content
    /// address.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(HEADER_LEN + self.chunks.len() * ENTRY_LEN);
        buf.extend_from_slice(MAGIC);
        buf.push(VERSION);
        buf.extend_from_slice(&(self.chunks.len() as u32).to_le_bytes());
        for chunk in &self.chunks {
            buf.extend_from_slice(chunk.hash.as_bytes());
            buf.extend_from_slice(&chunk.len.to_le_bytes());
        }
        buf
    }

    /// Decode a manifest previously produced by [`Manifest::encode`].
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            return Err(Error::Manifest("truncated header".into()));
        }
        if &bytes[0..4] != MAGIC {
            return Err(Error::Manifest("bad magic".into()));
        }
        let version = bytes[4];
        if version != VERSION {
            return Err(Error::Manifest(format!("unsupported version {version}")));
        }
        let count = u32::from_le_bytes(bytes[5..9].try_into().unwrap()) as usize;
        let expected_len = HEADER_LEN + count * ENTRY_LEN;
        if bytes.len() != expected_len {
            return Err(Error::Manifest(format!(
                "length mismatch: expected {expected_len} bytes for {count} chunks, got {}",
                bytes.len()
            )));
        }
        let mut chunks = Vec::with_capacity(count);
        let mut offset = HEADER_LEN;
        for _ in 0..count {
            let hash_bytes: [u8; 32] = bytes[offset..offset + 32].try_into().unwrap();
            let len_bytes: [u8; 8] = bytes[offset + 32..offset + 40].try_into().unwrap();
            chunks.push(ChunkRef {
                hash: blake3::Hash::from_bytes(hash_bytes),
                len: u64::from_le_bytes(len_bytes),
            });
            offset += ENTRY_LEN;
        }
        Ok(Self { chunks })
    }

    /// This manifest's content address — the `FileId` of the file it
    /// describes.
    pub fn file_id(&self) -> FileId {
        FileId(blake3::hash(&self.encode()))
    }
}

impl FileId {
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    pub fn to_hex(&self) -> String {
        self.0.to_hex().to_string()
    }

    pub fn from_hex(hex: &str) -> Result<Self> {
        blake3::Hash::from_hex(hex)
            .map(FileId)
            .map_err(|e| Error::Manifest(format!("bad file id hex: {e}")))
    }
}

impl std::fmt::Display for FileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(byte: u8, len: u64) -> ChunkRef {
        ChunkRef {
            hash: blake3::hash(&[byte]),
            len,
        }
    }

    #[test]
    fn round_trips_through_encode_decode() {
        let manifest = Manifest::new(vec![chunk(1, 100), chunk(2, 200), chunk(3, 300)]);
        let decoded = Manifest::decode(&manifest.encode()).unwrap();
        assert_eq!(manifest, decoded);
        assert_eq!(manifest.file_id(), decoded.file_id());
    }

    #[test]
    fn empty_manifest_round_trips() {
        let manifest = Manifest::new(vec![]);
        let decoded = Manifest::decode(&manifest.encode()).unwrap();
        assert_eq!(manifest, decoded);
        assert_eq!(manifest.total_len(), 0);
    }

    #[test]
    fn chunk_order_changes_file_id() {
        let a = Manifest::new(vec![chunk(1, 1), chunk(2, 1)]);
        let b = Manifest::new(vec![chunk(2, 1), chunk(1, 1)]);
        assert_ne!(a.file_id(), b.file_id());
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = Manifest::new(vec![chunk(1, 1)]).encode();
        bytes[0] = b'X';
        assert!(Manifest::decode(&bytes).is_err());
    }

    #[test]
    fn rejects_truncated_bytes() {
        let bytes = Manifest::new(vec![chunk(1, 1)]).encode();
        assert!(Manifest::decode(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn file_id_hex_round_trips() {
        let id = Manifest::new(vec![chunk(1, 1)]).file_id();
        assert_eq!(FileId::from_hex(&id.to_hex()).unwrap(), id);
    }
}
