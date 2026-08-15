//! End-to-end smoke test against a real `rust-analyzer` from PATH.
//!
//! `#[ignore]`d: it needs rust-analyzer installed and takes tens of
//! seconds while the server indexes. Run explicitly with
//! `cargo test -p editor-lsp -- --ignored`.

use std::str::FromStr;
use std::time::Duration;

use editor_lsp::{DiagnosticsStore, LspClient, ServerMessage, Severity, Transport, Uri};
use editor_state::Doc;

/// Spawn rust-analyzer on a throwaway cargo project containing a
/// type error, drive the didOpen flow, and wait for a non-empty
/// publishDiagnostics.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires rust-analyzer on PATH; slow (server indexing)"]
async fn rust_analyzer_publishes_type_error_diagnostics() {
    if !rust_analyzer_on_path() {
        eprintln!("rust-analyzer not on PATH — skipping");
        return;
    }

    // A minimal cargo project with a guaranteed type error.
    let root = std::env::temp_dir().join(format!("editor-lsp-ra-test-{}", std::process::id()));
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"ra-smoke\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let main_text = "fn main() {\n    let x: i32 = \"not a number\";\n}\n";
    let main_path = src.join("main.rs");
    std::fs::write(&main_path, main_text).unwrap();

    let transport = Transport::stdio("rust-analyzer", &[], Some(&root)).unwrap();
    let (client, mut events) = LspClient::new(transport);
    let root_uri = Uri::from_str(&format!("file://{}", root.display())).unwrap();
    client.initialize(Some(root_uri)).await.unwrap();

    let doc = Doc::from_str(main_text);
    let uri = Uri::from_str(&format!("file://{}", main_path.display())).unwrap();
    client.did_open(uri.clone(), "rust", &doc).await.unwrap();

    // Wait for the first non-empty diagnostics for our file.
    let mut store = DiagnosticsStore::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let diagnostic = loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("timed out waiting for diagnostics")
            .expect("server channel closed before diagnostics arrived");
        if let ServerMessage::Diagnostics(published) = event {
            if published.uri == uri
                && store.apply(&published, client.version_of(&uri), &doc)
                && !store.get(&uri).is_empty()
            {
                break store.get(&uri)[0].clone();
            }
        }
    };

    // The flagged byte range must lie inside the document and the
    // severity of a type mismatch is Error.
    assert!(diagnostic.to <= doc.len() && diagnostic.from < diagnostic.to);
    assert_eq!(diagnostic.severity, Severity::Error);

    client.shutdown().await.unwrap();
    std::fs::remove_dir_all(&root).ok();
}

fn rust_analyzer_on_path() -> bool {
    std::process::Command::new("rust-analyzer")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
