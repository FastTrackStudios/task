//! LSP wiring for the playground — the canonical host-side
//! integration of `editor-lsp` (see `docs/lsp.md`).
//!
//! The client runs on a dedicated thread with its own tokio
//! runtime, so it works identically under dioxus-desktop and the
//! Blitz native renderer (whose executor we don't control). The
//! UI talks to it over two channels:
//!
//! - editor → LSP: every `TransactionEvent` (the `on_transaction`
//!   sink) is forwarded; edits become incremental `didChange`.
//! - LSP → editor: each `publishDiagnostics` (and each local edit,
//!   which shifts the stored squiggles via `map_through`) sends a
//!   fresh `Vec<DecoratedRange>` the UI stores in a signal and
//!   splices into its `DecorationSource`.
//!
//! Enabled by env config — the playground doesn't guess at your
//! toolchain:
//!
//! ```sh
//! EDITOR_LSP_CMD="marksman server" cargo run -p playground
//! EDITOR_LSP_CMD="python3 tools/demo_ls.py" EDITOR_LSP_LANG=markdown \
//!     cargo run -p playground
//! ```
//!
//! `tools/demo_ls.py` is a self-contained demo server (flags
//! `TODO`/`FIXME`) so the pipeline is testable without installing
//! a real language server.

use editor::{DecoratedRange, Doc, TransactionEvent};
use editor_lsp::{DiagnosticsStore, LspClient, ServerMessage, Transport, Uri, to_decorations};
use tokio::sync::mpsc;

/// Handle the UI keeps. `events` is cloned into the
/// `on_transaction` callback; `decorations` is taken once by the
/// drain future.
pub struct LspBridge {
    pub events: mpsc::UnboundedSender<TransactionEvent>,
    pub decorations: Option<mpsc::UnboundedReceiver<Vec<DecoratedRange>>>,
}

/// Spawn the LSP thread if `EDITOR_LSP_CMD` is set. Returns `None`
/// (LSP disabled) otherwise — the playground works as before.
pub fn start(initial: Doc) -> Option<LspBridge> {
    let cmdline = std::env::var("EDITOR_LSP_CMD").ok()?;
    let lang = std::env::var("EDITOR_LSP_LANG").unwrap_or_else(|_| "markdown".to_owned());
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (deco_tx, deco_rx) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("lsp-client".into())
        .spawn(move || run(&cmdline, &lang, initial, events_rx, deco_tx))
        .ok()?;
    Some(LspBridge {
        events: events_tx,
        decorations: Some(deco_rx),
    })
}

fn run(
    cmdline: &str,
    lang: &str,
    initial: Doc,
    mut events: mpsc::UnboundedReceiver<TransactionEvent>,
    decos: mpsc::UnboundedSender<Vec<DecoratedRange>>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("lsp: tokio runtime failed: {e}");
            return;
        }
    };
    rt.block_on(async move {
        let mut parts = cmdline.split_whitespace();
        let Some(cmd) = parts.next() else {
            return;
        };
        let args: Vec<&str> = parts.collect();
        let cwd = std::env::current_dir().ok();
        let transport = match Transport::stdio(cmd, &args, cwd.as_deref()) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("lsp: failed to spawn `{cmdline}`: {e}");
                return;
            }
        };
        let (client, mut server_rx) = LspClient::new(transport);
        let uri: Uri = match "file:///playground.md".parse() {
            Ok(u) => u,
            Err(_) => return,
        };
        let root: Option<Uri> = cwd
            .as_deref()
            .and_then(|d| format!("file://{}", d.display()).parse().ok());
        if let Err(e) = client.initialize(root).await {
            tracing::error!("lsp: initialize failed: {e}");
            return;
        }
        if let Err(e) = client.did_open(uri.clone(), lang, &initial).await {
            tracing::error!("lsp: didOpen failed: {e}");
            return;
        }
        tracing::info!("lsp: `{cmdline}` up, document open");

        let mut store = DiagnosticsStore::new();
        let mut cur_doc = initial;
        loop {
            tokio::select! {
                ev = events.recv() => {
                    let Some(ev) = ev else {
                        // UI dropped the sender — app is closing.
                        let _ = client.shutdown().await;
                        break;
                    };
                    if !ev.is_edit() {
                        continue;
                    }
                    // Keep squiggles anchored locally until the
                    // server re-publishes.
                    store.map_through(&uri, &ev.changes);
                    let _ = decos.send(to_decorations(store.get(&uri)));
                    if let Err(e) = client.did_change(&uri, &ev.changes, &ev.doc_before).await {
                        tracing::error!("lsp: didChange failed: {e}");
                        break;
                    }
                    cur_doc = ev.doc_after.clone();
                }
                msg = server_rx.recv() => {
                    let Some(msg) = msg else {
                        tracing::warn!("lsp: server channel closed");
                        break;
                    };
                    match msg {
                        ServerMessage::Diagnostics(published) => {
                            if store.apply(&published, client.version_of(&uri), &cur_doc) {
                                let n = store.get(&uri).len();
                                tracing::debug!("lsp: {n} diagnostics");
                                let _ = decos.send(to_decorations(store.get(&uri)));
                            }
                        }
                        ServerMessage::Notification { method, .. } => {
                            tracing::trace!("lsp: server notification {method}");
                        }
                    }
                }
            }
        }
    });
}
