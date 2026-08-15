//! Recursive folder import. Walks a directory and lands
//! every file under `Wiki/raw/sources/`, optionally
//! preserving the relative subdirectory structure
//! (matches `llm_wiki`'s "folder context as classification
//! hint" feature).

use std::path::Path;

use mime_guess::MimeGuess;
use walkdir::WalkDir;
use wiki_proto::raw::{ImportRawSource, RawSourceRef};

use crate::error::WikiLiveError;
use crate::vault::WikiLive;

/// Options for [`WikiLive::import_folder`].
#[derive(Debug, Clone)]
pub struct ImportFolderOpts {
    /// Preserve relative directory structure under
    /// `Wiki/raw/sources/<sub>/...`. When `false`,
    /// everything lands flat with `_`-joined filenames.
    pub preserve_structure: bool,
    /// File extensions to include (lowercased, no dot).
    /// Empty = all files (except dotfiles).
    pub include_exts: Vec<String>,
    /// Substrings to exclude (matched against the relative
    /// path).
    pub exclude_substrings: Vec<String>,
}

impl Default for ImportFolderOpts {
    fn default() -> Self {
        Self {
            preserve_structure: true,
            include_exts: vec!["md".into(), "txt".into(), "pdf".into()],
            exclude_substrings: vec![".git/".into(), "node_modules/".into()],
        }
    }
}

impl WikiLive {
    /// Walk `dir` recursively, calling
    /// [`Self::import_raw_source`] for each file that
    /// passes the filters. Returns one `RawSourceRef` per
    /// imported entry, in directory-walk order.
    pub fn import_folder(
        &self,
        dir: &Path,
        opts: ImportFolderOpts,
    ) -> Result<Vec<RawSourceRef>, WikiLiveError> {
        if !dir.is_dir() {
            return Err(WikiLiveError::IllegalState(format!(
                "{} is not a directory",
                dir.display()
            )));
        }
        let mut out = Vec::new();
        for entry in WalkDir::new(dir).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let rel = path
                .strip_prefix(dir)
                .map_or_else(|_| name.to_string(), |p| p.to_string_lossy().to_string());
            if opts
                .exclude_substrings
                .iter()
                .any(|sub| rel.contains(sub.as_str()))
            {
                continue;
            }
            if !opts.include_exts.is_empty() {
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(str::to_ascii_lowercase)
                    .unwrap_or_default();
                if !opts.include_exts.contains(&ext) {
                    continue;
                }
            }
            let bytes = std::fs::read(path)?;
            let mime = MimeGuess::from_path(path)
                .first_or_octet_stream()
                .to_string();
            let filename = if opts.preserve_structure {
                rel.replace('\\', "/")
            } else {
                rel.replace(['/', '\\'], "_")
            };
            let raw_ref = self.import_raw_source(ImportRawSource {
                filename,
                mime,
                title: String::new(),
                bytes,
                auto_enqueue: false,
            })?;
            out.push(raw_ref);
        }
        Ok(out)
    }
}
