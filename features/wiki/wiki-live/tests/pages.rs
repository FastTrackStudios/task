//! `Pages` capability — curated page list / read / sha-guarded
//! write over a scratch wiki root.

use std::path::PathBuf;

use wiki_proto::WikiError;
use wiki_proto::service::Pages;
use wiki_live::WikiBackend;

/// Fresh scratch wiki root + backend. No tempfile dep in this
/// crate — a pid+counter-suffixed dir under the target tmpdir is
/// enough, cleaned by the returned guard.
fn scratch() -> (WikiBackend, PathBuf, DirGuard) {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "wiki-live-pages-test-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let backend = WikiBackend::single("default", root.clone()).expect("backend");
    (backend, root.clone(), DirGuard(root))
}

struct DirGuard(PathBuf);
impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn write_read_list_roundtrip() {
    let (b, root, _g) = scratch();

    let doc = b
        .write_page(
            "default",
            "Concepts/Spaced repetition.md",
            "---\ntitle: \"Spaced repetition\"\ntype: concept\n---\n\nBody.\n",
            "",
        )
        .expect("write");
    assert_eq!(doc.path, "Concepts/Spaced repetition.md");
    assert!(!doc.sha256.is_empty());
    assert!(root.join("Concepts/Spaced repetition.md").is_file());

    let read = b
        .read_page("default", "Concepts/Spaced repetition.md")
        .expect("read");
    assert_eq!(read.markdown, doc.markdown);
    assert_eq!(read.sha256, doc.sha256);

    let pages = b.list_pages("default").expect("list");
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].title, "Spaced repetition");
    assert_eq!(pages[0].page_type, "concept");
}

#[test]
fn stale_sha_write_conflicts_and_fresh_sha_wins() {
    let (b, _root, _g) = scratch();
    let v1 = b
        .write_page("default", "Entities/Foo.md", "# Foo v1\n", "")
        .expect("create");

    // Guarded write with the current sha succeeds.
    let v2 = b
        .write_page("default", "Entities/Foo.md", "# Foo v2\n", &v1.sha256)
        .expect("guarded write");

    // Reusing the stale v1 sha is a conflict.
    let err = b
        .write_page("default", "Entities/Foo.md", "# Foo v3\n", &v1.sha256)
        .expect_err("stale sha must conflict");
    assert!(matches!(err, WikiError::IllegalState(_)), "got {err:?}");

    // The file still holds v2.
    let read = b.read_page("default", "Entities/Foo.md").expect("read");
    assert_eq!(read.markdown, "# Foo v2\n");
    assert_eq!(read.sha256, v2.sha256);
}

#[test]
fn list_excludes_raw_state_media_and_read_rejects_them() {
    let (b, root, _g) = scratch();
    b.write_page("default", "Concepts/Keep.md", "# Keep\n", "")
        .expect("page");
    for hidden in ["raw/sources/src.md", "_state/notes.md", "media/img.md"] {
        let abs = root.join(hidden);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, "# hidden\n").unwrap();
    }

    let pages = b.list_pages("default").expect("list");
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].path, "Concepts/Keep.md");

    assert!(b.read_page("default", "raw/sources/src.md").is_err());
    assert!(b.write_page("default", "_state/notes.md", "x", "").is_err());
}

#[test]
fn path_traversal_and_non_markdown_rejected() {
    let (b, _root, _g) = scratch();
    for bad in ["../escape.md", "a/../../escape.md", "/abs.md", "note.txt", ""] {
        assert!(
            b.write_page("default", bad, "x", "").is_err(),
            "`{bad}` must be rejected"
        );
        assert!(b.read_page("default", bad).is_err());
    }
}
