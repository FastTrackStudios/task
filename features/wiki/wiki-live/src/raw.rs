//! Raw layer — `Wiki/raw/sources/`. Immutable from the
//! wiki's perspective; bytes only land via
//! [`WikiLive::import_raw_source`] and only leave via
//! [`WikiLive::delete_raw_source`].

use std::fs;
use std::io::Write;

use chrono::Utc;
use sha2::{Digest, Sha256};
use wiki_proto::paths;
use wiki_proto::raw::{ImportRawSource, RawSourceRef};

use crate::error::WikiLiveError;
use crate::vault::WikiLive;

pub(crate) fn bootstrap_dirs(wiki: &WikiLive) -> Result<(), WikiLiveError> {
    let root = wiki.wiki_root();
    fs::create_dir_all(root.join(paths::SOURCES_DIR))?;
    fs::create_dir_all(root.join(paths::MEDIA_DIR))?;
    fs::create_dir_all(root.join(paths::STATE_DIR))?;
    Ok(())
}

impl WikiLive {
    /// Persist bytes under `Wiki/raw/sources/<filename>`.
    /// Collisions get a `-<sha8>` suffix. **SHA256 dedupe:**
    /// if an existing file in `sources/` has byte-for-byte
    /// identical content, returns the existing `RawSourceRef`
    /// instead of writing a second copy (matches the
    /// nashsu/llm_wiki incremental cache — unchanged sources
    /// are skipped to save LLM tokens).
    pub fn import_raw_source(
        &self,
        source: ImportRawSource,
    ) -> Result<RawSourceRef, WikiLiveError> {
        let sources_dir = self.wiki_root().join(paths::SOURCES_DIR);
        fs::create_dir_all(&sources_dir)?;

        let hash = sha256_hex(&source.bytes);

        // Dedupe: scan the sources dir and return early if any
        // file hashes to the same SHA256 as the incoming bytes.
        // Walk just the immediate directory (sources are flat by
        // convention; folder-import is a separate verb).
        if let Ok(read) = fs::read_dir(&sources_dir) {
            for entry in read.flatten() {
                let p = entry.path();
                if !p.is_file() {
                    continue;
                }
                let Ok(existing_bytes) = fs::read(&p) else {
                    continue;
                };
                if sha256_hex(&existing_bytes) == hash {
                    let filename = p
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string();
                    let rel = format!("{}/{}", paths::SOURCES_DIR, filename);
                    return Ok(RawSourceRef {
                        path: rel,
                        filename,
                        mime: source.mime,
                        size: existing_bytes.len() as u64,
                        sha256: hash,
                        imported_at: Utc::now(),
                        title: source.title,
                    });
                }
            }
        }

        let safe_name = sanitize_filename(&source.filename);
        let target = unique_path(&sources_dir, &safe_name, &hash);
        let abs = sources_dir.join(&target);

        // Atomic write.
        let tmp = abs.with_extension(format!("tmp.{}", uuid::Uuid::new_v4().simple()));
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&source.bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, &abs)?;

        let rel = format!("{}/{}", paths::SOURCES_DIR, target);
        Ok(RawSourceRef {
            path: rel,
            filename: target,
            mime: source.mime,
            size: source.bytes.len() as u64,
            sha256: hash,
            imported_at: Utc::now(),
            title: source.title,
        })
    }

    /// Read a raw source's bytes back. `rel_path` is
    /// vault-relative — typically the value returned in
    /// `RawSourceRef::path`.
    pub fn read_raw_source(&self, rel_path: &str) -> Result<Vec<u8>, WikiLiveError> {
        let abs = self.wiki_root().join(rel_path);
        Ok(fs::read(abs)?)
    }
}

fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("source");
    }
    out
}

fn unique_path(dir: &std::path::Path, name: &str, hash: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let stem;
    let ext;
    if let Some(dot) = name.rfind('.') {
        stem = &name[..dot];
        ext = &name[dot..];
    } else {
        stem = name;
        ext = "";
    }
    format!("{stem}-{}{ext}", &hash[..8])
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Test whether a relative path lives under the immutable
/// raw layer. Used by [`super::WikiLive::record_pages`] to
/// reject writes that would corrupt sources.
pub(crate) fn is_raw_path(rel: &str) -> bool {
    let prefix = format!("{}/", paths::SOURCES_DIR);
    let prefix_dot = format!("./{prefix}");
    let raw_prefix = "raw/";
    rel.starts_with(&prefix) || rel.starts_with(&prefix_dot) || rel.starts_with(raw_prefix)
}
