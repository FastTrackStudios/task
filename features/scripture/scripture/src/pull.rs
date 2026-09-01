//! Pulling a public-domain edition in whole — download, unzip, install.
//!
//! [`install_usfm_dir`](crate::install_usfm_dir) is still the only
//! thing that decides what a book is and where it lands; everything
//! here exists to hand it a directory. A zip is unpacked to a temp dir
//! and installed from there, so a corrupt archive cannot leave a
//! half-written corpus behind the reader would serve as if whole.
//!
//! Downloads are cached by edition, because planting a demo twice
//! should not fetch fifty megabytes twice, and because a machine with
//! no network still has to be able to plant a world it has planted
//! before.

use std::path::{Path, PathBuf};

use scripture_proto::Book;

use crate::install::{InstallError, install_usfm_dir};
use crate::sources::{Source, SourceError, source_for};

#[derive(Debug, thiserror::Error)]
pub enum PullError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error("download {url}: {source}")]
    Fetch {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("unpack {id}: {source}")]
    Unzip {
        id: String,
        #[source]
        source: zip::result::ZipError,
    },
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> PullError + '_ {
    move |source| PullError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Where downloaded archives are kept between plants.
///
/// `$TASK_BIBLE_CACHE` → `$XDG_CACHE_HOME/task/bibles` →
/// `$HOME/.cache/task/bibles`. Outside any data root on purpose: the
/// archive is not org state, and one machine planting five demo orgs
/// should download once.
#[must_use]
pub fn cache_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var("TASK_BIBLE_CACHE") {
        if !explicit.trim().is_empty() {
            return PathBuf::from(explicit);
        }
    }
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map_or_else(
            || {
                std::env::var("HOME").map_or_else(
                    |_| PathBuf::from(".cache"),
                    |h| PathBuf::from(h).join(".cache"),
                )
            },
            PathBuf::from,
        );
    base.join("task").join("bibles")
}

/// The cached archive for an edition, if one has been downloaded.
#[must_use]
pub fn cached(source: Source) -> Option<PathBuf> {
    let path = cache_dir().join(format!("{}.zip", source.id));
    path.is_file().then_some(path)
}

/// What a pull did — worth reporting, since "installed 66 books" and
/// "found it already there" look identical from the outside.
#[derive(Debug, Clone)]
pub struct Pulled {
    pub id: String,
    pub books: Vec<Book>,
    /// Whether the archive came from the cache rather than the network.
    pub from_cache: bool,
}

/// Install a public-domain edition into `dest_dir`, downloading it if
/// it is not already cached.
///
/// Refuses a licensed edition — see [`source_for`]. `dest_dir` is the
/// translation folder itself (`<org>/resources/bible/WEB`).
///
/// # Errors
///
/// A licensed or unknown edition, a failed download, a corrupt
/// archive, or any filesystem error writing the corpus.
pub async fn pull(id: &str, dest_dir: &Path) -> Result<Pulled, PullError> {
    let source = source_for(id)?;
    let (archive, from_cache) = match cached(source) {
        Some(path) => (std::fs::read(&path).map_err(io(&path))?, true),
        None => {
            let bytes = download(source).await?;
            let dir = cache_dir();
            std::fs::create_dir_all(&dir).map_err(io(&dir))?;
            let path = dir.join(format!("{}.zip", source.id));
            std::fs::write(&path, &bytes).map_err(io(&path))?;
            (bytes, false)
        }
    };
    let books = install_archive(source, &archive, dest_dir)?;
    Ok(Pulled {
        id: source.id.to_owned(),
        books,
        from_cache,
    })
}

/// Install from an archive already on disk — the offline path, and
/// what a test uses.
///
/// # Errors
///
/// As [`pull`], minus the download.
pub fn pull_from_archive(id: &str, archive: &Path, dest_dir: &Path) -> Result<Pulled, PullError> {
    let source = source_for(id)?;
    let bytes = std::fs::read(archive).map_err(io(archive))?;
    let books = install_archive(source, &bytes, dest_dir)?;
    Ok(Pulled {
        id: source.id.to_owned(),
        books,
        from_cache: true,
    })
}

async fn download(source: Source) -> Result<Vec<u8>, PullError> {
    let fetch = |source: Source| async move {
        let response = reqwest::get(source.url).await?.error_for_status()?;
        response.bytes().await
    };
    fetch(source)
        .await
        .map(|b| b.to_vec())
        .map_err(|e| PullError::Fetch {
            url: source.url.to_owned(),
            source: e,
        })
}

/// Unpack to a temp dir, then install from it.
///
/// Two-step rather than streaming each entry into place so a bad
/// archive fails before anything lands in the resource library:
/// `install_usfm_dir` is the one place that decides what a book is,
/// and it should see a whole directory or none.
fn install_archive(
    source: Source,
    archive: &[u8],
    dest_dir: &Path,
) -> Result<Vec<Book>, PullError> {
    let staging = tempfile::tempdir().map_err(io(Path::new("(temp dir)")))?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).map_err(|source_err| {
        PullError::Unzip {
            id: source.id.to_owned(),
            source: source_err,
        }
    })?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|source_err| PullError::Unzip {
            id: source.id.to_owned(),
            source: source_err,
        })?;
        if !entry.is_file() {
            continue;
        }
        // Flatten: `install_usfm_dir` reads one directory, and an
        // archive's internal layout is not our business. Take the file
        // name only, which also means a crafted archive cannot write
        // outside the staging dir.
        let Some(name) = entry.enclosed_name().and_then(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(std::borrow::ToOwned::to_owned)
        }) else {
            continue;
        };
        if !Path::new(&name)
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("usfm"))
        {
            continue;
        }
        let out = staging.path().join(&name);
        let mut file = std::fs::File::create(&out).map_err(io(&out))?;
        std::io::copy(&mut entry, &mut file).map_err(io(&out))?;
    }
    Ok(install_usfm_dir(staging.path(), dest_dir)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_licensed_edition_is_refused_before_any_network_call() {
        let tmp = tempfile::tempdir().unwrap();
        let err = pull("NIV", tmp.path()).await.expect_err("must refuse");
        assert!(matches!(
            err,
            PullError::Source(SourceError::Licensed { .. })
        ));
        // Nothing was written: a refusal must not leave a corpus dir
        // behind for the reader to find and half-serve.
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }

    /// The real thing, against eBible.org. Ignored by default: the
    /// suite must not need a network, and a fetch of this size does
    /// not belong in every run. `cargo test -p scripture -- --ignored`
    /// when the sources need re-checking.
    #[tokio::test]
    #[ignore = "downloads a corpus"]
    async fn web_pulls_and_reads_back() {
        let dest = tempfile::tempdir().unwrap();
        let pulled = pull("WEB", dest.path()).await.expect("pull WEB");
        assert_eq!(pulled.books.len(), 66, "a whole canon");
        let bible = crate::Bible::load_dir(dest.path(), "WEB").expect("load back");
        let john = scripture_proto::VerseId::parse("John 3:16").unwrap();
        assert!(bible.get(john).is_some_and(|v| !v.trim().is_empty()));
    }

    #[test]
    fn an_archive_installs_only_its_canonical_books() {
        let staging = tempfile::tempdir().unwrap();
        let zip_path = staging.path().join("web.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut w = zip::ZipWriter::new(file);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            use std::io::Write;
            // A nested path, to prove flattening.
            w.start_file("release/USFM/41-MATeng-web.usfm", opts)
                .unwrap();
            w.write_all(b"\\id MAT\n\\c 1\n\\v 1 A record.\n").unwrap();
            // Front matter, which is not a book.
            w.start_file("release/USFM/00-FRTeng-web.usfm", opts)
                .unwrap();
            w.write_all(b"\\id FRT\n\\p About this edition.\n").unwrap();
            // Not USFM at all.
            w.start_file("release/copr.htm", opts).unwrap();
            w.write_all(b"<html>public domain</html>").unwrap();
            w.finish().unwrap();
        }
        let dest = tempfile::tempdir().unwrap();
        let pulled = pull_from_archive("WEB", &zip_path, dest.path()).unwrap();
        assert_eq!(pulled.books.len(), 1, "only the canonical book installs");
        assert!(dest.path().join("MAT.usfm").is_file());
        assert!(!dest.path().join("FRT.usfm").exists());
    }
}
