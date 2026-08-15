//! Host-side LSP client for the editor: spawn a language server,
//! keep it fed with incremental document edits, and surface its
//! diagnostics as byte-range data the view can decorate.
//!
//! Pure tokio + `editor-state` — no dioxus, no editor-view. The
//! *host* (the app embedding `<Editor>`) owns an [`LspClient`] and
//! wires it to the editor's transaction stream; see `docs/lsp.md`
//! for the architecture and the canonical wiring. Runs on
//! desktop/native today; the transport abstraction (a channel pair,
//! see [`transport`]) leaves room for a websocket proxy so wasm
//! hosts can join later.
//!
//! Layering, bottom-up:
//!
//! - [`transport`] — `Content-Length`-framed JSON-RPC codec plus the
//!   stdio child-process backend.
//! - [`pos`] — byte offset ↔ UTF-16 line/character conversion, and
//!   `Changes` → incremental `didChange` translation. The
//!   correctness-critical seam.
//! - [`client`] — lifecycle (`initialize`/`shutdown`), document sync
//!   (`didOpen`/`didChange`/`didClose` with version tracking),
//!   request/response correlation, server-push channel.
//! - [`diagnostics`] — `publishDiagnostics` → byte-range
//!   [`Diagnostic`]s with stale-version filtering, plus the
//!   `DecoratedRange` mapping.
//!
//! Reference: the LSP 3.17 specification ("Base Protocol" and
//! "Text Document Synchronization" sections); CM6's lint package
//! (`~/Development/research/codemirror`) for the
//! diagnostics-as-decorations shape.

pub mod client;
pub mod diagnostics;
pub mod pos;
pub mod transport;

pub use client::{Error, LspClient, ServerMessage};
pub use diagnostics::{
    Diagnostic, DiagnosticsStore, PublishedDiagnostics, Severity, to_decorations,
};
pub use pos::{byte_to_position, changes_to_content_changes, position_to_byte};
pub use transport::Transport;

// Re-exported so hosts can name URIs and server capabilities without
// adding their own lsp-types dependency (and risking a version skew
// with ours).
pub use lsp_types;
pub use lsp_types::Uri;
