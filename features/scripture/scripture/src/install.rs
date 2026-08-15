//! Install a Bible translation into the resource library.
//!
//! Source USFM ships with noisy, numbered filenames
//! (`73-JHNengwebp.usfm`) and front/back-matter files that aren't books.
//! [`install_usfm_dir`] normalizes a source directory into a clean
//! translation folder — one file per canonical book, named by its
//! `USFM` id (`JHN.usfm`) — ready for [`crate::Bible::load_dir`].
//!
//! The raw, fully-tagged USFM is copied verbatim (the `\w …|strong=…\w*`
//! word tags are preserved for later word study); cleaning happens at
//! read time, not install time.

use std::path::Path;

use scripture_proto::Book;

use crate::usfm::detect_book;

/// Failure installing a translation directory.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("io on {path:?}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> InstallError + '_ {
    move |source| InstallError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Copy every recognizable book USFM from `src_dir` into `dest_dir`,
/// renamed to `<USFM_ID>.usfm`. Files whose `\id` isn't a canonical book
/// (front/back matter, glossaries) are skipped. Returns the books
/// installed, in canonical order.
pub fn install_usfm_dir(src_dir: &Path, dest_dir: &Path) -> Result<Vec<Book>, InstallError> {
    std::fs::create_dir_all(dest_dir).map_err(io(dest_dir))?;

    let mut installed = Vec::new();
    let entries = std::fs::read_dir(src_dir).map_err(io(src_dir))?;
    for entry in entries {
        let path = entry.map_err(io(src_dir))?.path();
        if !path
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("usfm"))
        {
            continue;
        }
        let src = std::fs::read_to_string(&path).map_err(io(&path))?;
        // Skip anything that isn't a canonical book (front matter, etc.).
        let Ok(book) = detect_book(&src) else {
            continue;
        };
        let dest = dest_dir.join(format!("{}.usfm", book.usfm()));
        std::fs::write(&dest, &src).map_err(io(&dest))?;
        installed.push(book);
    }
    installed.sort();
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_books_and_skips_front_matter() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        // A book with a noisy name, plus a front-matter file to skip.
        std::fs::write(
            src.path().join("73-JHNengwebp.usfm"),
            crate::usfm::tests::SAMPLE,
        )
        .unwrap();
        std::fs::write(
            src.path().join("00-FRTengwebp.usfm"),
            "\\id FRT front matter\n",
        )
        .unwrap();

        let installed = install_usfm_dir(src.path(), dest.path()).unwrap();
        assert_eq!(installed, vec![Book::lookup("John").unwrap()]);
        // Normalized filename, raw tags preserved.
        let written = std::fs::read_to_string(dest.path().join("JHN.usfm")).unwrap();
        assert!(
            written.contains(r#"strong="G1063""#),
            "word tags preserved verbatim"
        );
        assert!(
            !dest.path().join("FRT.usfm").exists(),
            "front matter skipped"
        );
    }
}
