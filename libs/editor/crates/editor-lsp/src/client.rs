//! The LSP client — lifecycle, document sync, and message routing.
//!
//! An [`LspClient`] wraps a [`Transport`] (any backend — see
//! `transport.rs`) with the JSON-RPC bookkeeping a host needs:
//!
//! - **Correlation**: each outgoing request gets a fresh numeric id
//!   and a oneshot; a router task matches the server's `Response.id`
//!   back to the waiting caller.
//! - **Lifecycle**: [`initialize`](LspClient::initialize) performs
//!   the `initialize` → `initialized` handshake (advertising
//!   incremental sync + versioned diagnostics),
//!   [`shutdown`](LspClient::shutdown) the `shutdown` → `exit` pair.
//! - **Document sync**: [`did_open`](LspClient::did_open) /
//!   [`did_change`](LspClient::did_change) /
//!   [`did_close`](LspClient::did_close), with per-document version
//!   counters. `did_change` is *incremental*: it takes the same
//!   `Changes` + `doc_before` pair a `TransactionEvent` carries and
//!   translates it through [`crate::pos`] — the whole document is
//!   never re-sent.
//! - **Server pushes**: `publishDiagnostics` and other notifications
//!   surface on the [`ServerMessage`] channel returned by
//!   [`LspClient::new`]. Server→client *requests* we don't implement
//!   are auto-answered (null for `window/workDoneProgress/create`,
//!   `MethodNotFound` otherwise) so the server never hangs on us.
//!
//! No dioxus, no view types: the host wires this next to its editor
//! however it likes (see `docs/lsp.md` for the canonical wiring).

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use editor_state::{Changes, Doc};
use lsp_types::notification::Notification as NotificationTrait;
use lsp_types::request::Request as RequestTrait;
use lsp_types::{
    ClientCapabilities, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, InitializeParams, InitializeResult, InitializedParams,
    PublishDiagnosticsClientCapabilities, PublishDiagnosticsParams,
    TextDocumentClientCapabilities, TextDocumentIdentifier, TextDocumentItem, Uri,
    VersionedTextDocumentIdentifier, WorkspaceFolder,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::diagnostics::PublishedDiagnostics;
use crate::pos::changes_to_content_changes;
use crate::transport::{
    METHOD_NOT_FOUND, Message, Notification, Request, Response, ResponseError, Transport,
};

/// Client-side failures. Server-reported request errors come through
/// as [`Error::Server`] with the JSON-RPC error object intact.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("transport closed (server exited or connection dropped)")]
    Closed,
    #[error("server error {code}: {message}", code = .0.code, message = .0.message)]
    Server(ResponseError),
    #[error("serialization: {0}")]
    Json(#[from] serde_json::Error),
    #[error("document not open: {0}")]
    NotOpen(String),
}

/// A server-initiated message, surfaced to the host on the channel
/// [`LspClient::new`] returns. Diagnostics get a typed variant (they
/// are the crate's raison d'être); everything else passes through
/// raw for the host — or a future hover/completion layer — to
/// inspect.
#[derive(Debug)]
pub enum ServerMessage {
    /// `textDocument/publishDiagnostics`. Positions are still LSP
    /// line/character — resolve against the *current* doc via
    /// [`crate::diagnostics::DiagnosticsStore::apply`].
    Diagnostics(PublishedDiagnostics),
    /// Any other server notification (`window/logMessage`,
    /// `$/progress`, …).
    Notification { method: String, params: Value },
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, ResponseError>>>>>;

/// Handle to a running language server. Owned by the host; all
/// methods are async and safe to call from any task. Dropping the
/// client tears down the router and (for the stdio backend) kills
/// the child process.
pub struct LspClient {
    outgoing: mpsc::Sender<Message>,
    pending: Pending,
    next_id: AtomicI64,
    /// Per-document `didChange` version counters, keyed by URI.
    versions: Mutex<HashMap<String, i32>>,
}

impl LspClient {
    /// Wrap a transport. Returns the client plus the channel on
    /// which server pushes ([`ServerMessage`]) arrive; the host
    /// drains that channel from its own task/loop.
    ///
    /// Spawns the router task, so must be called within a tokio
    /// runtime.
    #[must_use]
    pub fn new(transport: Transport) -> (Self, mpsc::Receiver<ServerMessage>) {
        let Transport {
            outgoing,
            mut incoming,
        } = transport;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, events_rx) = mpsc::channel::<ServerMessage>(64);

        // Router: demux everything the server sends.
        let router_pending = Arc::clone(&pending);
        let router_outgoing = outgoing.clone();
        tokio::spawn(async move {
            while let Some(msg) = incoming.recv().await {
                match msg {
                    Message::Response(resp) => {
                        let waiter = resp
                            .id
                            .as_i64()
                            .and_then(|id| router_pending.lock().unwrap().remove(&id));
                        if let Some(tx) = waiter {
                            let outcome = match resp.error {
                                Some(err) => Err(err),
                                None => Ok(resp.result.unwrap_or(Value::Null)),
                            };
                            let _ = tx.send(outcome);
                        }
                    }
                    Message::Notification(n) => {
                        let event = route_notification(n);
                        if events_tx.send(event).await.is_err() {
                            break; // host hung up
                        }
                    }
                    Message::Request(req) => {
                        // Answer server→client requests so the server
                        // never blocks on us.
                        let response = answer_server_request(&req);
                        if router_outgoing
                            .send(Message::Response(response))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            // Transport closed: fail every in-flight request.
            router_pending.lock().unwrap().clear();
        });

        (
            Self {
                outgoing,
                pending,
                next_id: AtomicI64::new(1),
                versions: Mutex::new(HashMap::new()),
            },
            events_rx,
        )
    }

    /// Send a typed request and await its typed response.
    pub async fn request<R: RequestTrait>(&self, params: R::Params) -> Result<R::Result, Error> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let msg = Message::Request(Request {
            jsonrpc: "2.0".to_owned(),
            id: json!(id),
            method: R::METHOD.to_owned(),
            params: serde_json::to_value(params)?,
        });
        if self.outgoing.send(msg).await.is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(Error::Closed);
        }
        let value = rx
            .await
            .map_err(|_| Error::Closed)?
            .map_err(Error::Server)?;
        Ok(serde_json::from_value(value)?)
    }

    /// Send a typed notification (no response expected).
    pub async fn notify<N: NotificationTrait>(&self, params: N::Params) -> Result<(), Error> {
        let msg = Message::Notification(Notification {
            jsonrpc: "2.0".to_owned(),
            method: N::METHOD.to_owned(),
            params: serde_json::to_value(params)?,
        });
        self.outgoing.send(msg).await.map_err(|_| Error::Closed)
    }

    /// The `initialize` → `initialized` handshake. Advertises the
    /// capabilities this crate actually consumes: incremental text
    /// sync and version-stamped `publishDiagnostics`. Returns the
    /// server's capabilities for the host to inspect (e.g. before
    /// wiring hover/completion later).
    pub async fn initialize(
        &self,
        workspace_root: Option<Uri>,
    ) -> Result<InitializeResult, Error> {
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                        version_support: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            workspace_folders: workspace_root.map(|uri| {
                vec![WorkspaceFolder {
                    name: "root".to_owned(),
                    uri,
                }]
            }),
            ..Default::default()
        };
        let result = self
            .request::<lsp_types::request::Initialize>(params)
            .await?;
        self.notify::<lsp_types::notification::Initialized>(InitializedParams {})
            .await?;
        Ok(result)
    }

    /// Graceful teardown: `shutdown` request, then `exit`
    /// notification. The stdio backend reaps the child when its
    /// stdout closes (and kills it on drop regardless).
    pub async fn shutdown(&self) -> Result<(), Error> {
        self.request::<lsp_types::request::Shutdown>(()).await?;
        self.notify::<lsp_types::notification::Exit>(()).await
    }

    /// Open a document (version 1). `language_id` is the LSP
    /// language identifier (`"rust"`, `"markdown"`, …).
    pub async fn did_open(&self, uri: Uri, language_id: &str, doc: &Doc) -> Result<(), Error> {
        self.versions.lock().unwrap().insert(uri.to_string(), 1);
        self.notify::<lsp_types::notification::DidOpenTextDocument>(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: language_id.to_owned(),
                version: 1,
                text: doc.to_string(),
            },
        })
        .await
    }

    /// Report an edit incrementally. `changes` and `doc_before` are
    /// exactly what a `TransactionEvent` carries (`event.changes`,
    /// `event.doc_before`); the byte offsets are translated to
    /// UTF-16 content-change events against `doc_before`. Bumps and
    /// returns the document version — the number stale diagnostics
    /// are filtered against.
    pub async fn did_change(
        &self,
        uri: &Uri,
        changes: &Changes,
        doc_before: &Doc,
    ) -> Result<i32, Error> {
        let version = {
            let mut versions = self.versions.lock().unwrap();
            let v = versions
                .get_mut(&uri.to_string())
                .ok_or_else(|| Error::NotOpen(uri.to_string()))?;
            *v += 1;
            *v
        };
        self.notify::<lsp_types::notification::DidChangeTextDocument>(
            DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                content_changes: changes_to_content_changes(doc_before, changes),
            },
        )
        .await?;
        Ok(version)
    }

    /// Close a document and forget its version counter.
    pub async fn did_close(&self, uri: &Uri) -> Result<(), Error> {
        self.versions.lock().unwrap().remove(&uri.to_string());
        self.notify::<lsp_types::notification::DidCloseTextDocument>(DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
        })
        .await
    }

    /// The last `didChange` version sent for a document — what the
    /// host passes to `DiagnosticsStore::apply` as `current_version`.
    #[must_use]
    pub fn version_of(&self, uri: &Uri) -> Option<i32> {
        self.versions.lock().unwrap().get(&uri.to_string()).copied()
    }
}

/// Turn a server notification into the host-facing event.
fn route_notification(n: Notification) -> ServerMessage {
    if n.method == lsp_types::notification::PublishDiagnostics::METHOD {
        if let Ok(params) = serde_json::from_value::<PublishDiagnosticsParams>(n.params.clone()) {
            return ServerMessage::Diagnostics(PublishedDiagnostics {
                uri: params.uri,
                version: params.version,
                diagnostics: params.diagnostics,
            });
        }
    }
    ServerMessage::Notification {
        method: n.method,
        params: n.params,
    }
}

/// Minimal answers to server→client requests. `workDoneProgress/
/// create` gets the null success it expects; anything else gets
/// `MethodNotFound`, which spec-conformant servers handle gracefully.
fn answer_server_request(req: &Request) -> Response {
    match req.method.as_str() {
        "window/workDoneProgress/create" => Response {
            jsonrpc: "2.0".to_owned(),
            id: req.id.clone(),
            result: Some(Value::Null),
            error: None,
        },
        _ => Response {
            jsonrpc: "2.0".to_owned(),
            id: req.id.clone(),
            result: None,
            error: Some(ResponseError {
                code: METHOD_NOT_FOUND,
                message: format!("client does not implement {}", req.method),
                data: None,
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn uri() -> Uri {
        Uri::from_str("file:///tmp/main.rs").unwrap()
    }

    /// A loopback harness: the test plays the language server on the
    /// far side of a channel-pair transport (the same seam a
    /// websocket proxy would plug into).
    fn loopback() -> (
        LspClient,
        mpsc::Receiver<ServerMessage>,
        mpsc::Receiver<Message>, // what the client sent
        mpsc::Sender<Message>,   // inject server messages
    ) {
        let (out_tx, out_rx) = mpsc::channel(32);
        let (in_tx, in_rx) = mpsc::channel(32);
        let (client, events) = LspClient::new(Transport::from_channels(out_tx, in_rx));
        (client, events, out_rx, in_tx)
    }

    #[tokio::test]
    async fn initialize_handshake_roundtrips() {
        let (client, _events, mut sent, server) = loopback();
        // Fake server: answer `initialize`, observe `initialized`.
        let server_task = tokio::spawn(async move {
            let Some(Message::Request(req)) = sent.recv().await else {
                panic!("expected initialize request");
            };
            assert_eq!(req.method, "initialize");
            assert_eq!(req.params["processId"], json!(std::process::id()));
            server
                .send(Message::Response(Response {
                    jsonrpc: "2.0".to_owned(),
                    id: req.id,
                    result: Some(json!({"capabilities": {}})),
                    error: None,
                }))
                .await
                .unwrap();
            let Some(Message::Notification(n)) = sent.recv().await else {
                panic!("expected initialized notification");
            };
            assert_eq!(n.method, "initialized");
            sent
        });
        client.initialize(None).await.unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn did_open_change_close_track_versions() {
        let (client, _events, mut sent, _server) = loopback();
        let doc = Doc::from_str("fn main() {}\n");
        client.did_open(uri(), "rust", &doc).await.unwrap();
        assert_eq!(client.version_of(&uri()), Some(1));

        let v = client
            .did_change(&uri(), &Changes::insert(11, "!"), &doc)
            .await
            .unwrap();
        assert_eq!(v, 2);
        assert_eq!(client.version_of(&uri()), Some(2));

        // Inspect what went over the wire.
        let Some(Message::Notification(open)) = sent.recv().await else {
            panic!("expected didOpen");
        };
        assert_eq!(open.method, "textDocument/didOpen");
        assert_eq!(open.params["textDocument"]["version"], json!(1));
        let Some(Message::Notification(change)) = sent.recv().await else {
            panic!("expected didChange");
        };
        assert_eq!(change.method, "textDocument/didChange");
        assert_eq!(change.params["textDocument"]["version"], json!(2));
        assert_eq!(change.params["contentChanges"][0]["text"], json!("!"));
        assert_eq!(
            change.params["contentChanges"][0]["range"]["start"],
            json!({"line": 0, "character": 11})
        );

        client.did_close(&uri()).await.unwrap();
        assert_eq!(client.version_of(&uri()), None);
    }

    #[tokio::test]
    async fn did_change_on_unopened_doc_errors() {
        let (client, _events, _sent, _server) = loopback();
        let doc = Doc::from_str("x");
        let err = client
            .did_change(&uri(), &Changes::insert(0, "y"), &doc)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NotOpen(_)));
    }

    #[tokio::test]
    async fn publish_diagnostics_surfaces_as_typed_event() {
        let (_client, mut events, _sent, server) = loopback();
        server
            .send(Message::Notification(Notification {
                jsonrpc: "2.0".to_owned(),
                method: "textDocument/publishDiagnostics".to_owned(),
                params: json!({
                    "uri": "file:///tmp/main.rs",
                    "version": 3,
                    "diagnostics": [{
                        "range": {"start": {"line": 0, "character": 0},
                                   "end": {"line": 0, "character": 2}},
                        "message": "mismatched types",
                        "severity": 1
                    }]
                }),
            }))
            .await
            .unwrap();
        let Some(ServerMessage::Diagnostics(published)) = events.recv().await else {
            panic!("expected diagnostics event");
        };
        assert_eq!(published.version, Some(3));
        assert_eq!(published.diagnostics.len(), 1);
        assert_eq!(published.diagnostics[0].message, "mismatched types");
    }

    #[tokio::test]
    async fn other_notifications_pass_through_raw() {
        let (_client, mut events, _sent, server) = loopback();
        server
            .send(Message::Notification(Notification {
                jsonrpc: "2.0".to_owned(),
                method: "window/logMessage".to_owned(),
                params: json!({"type": 3, "message": "indexing"}),
            }))
            .await
            .unwrap();
        let Some(ServerMessage::Notification { method, params }) = events.recv().await else {
            panic!("expected raw notification");
        };
        assert_eq!(method, "window/logMessage");
        assert_eq!(params["message"], json!("indexing"));
    }

    #[tokio::test]
    async fn server_requests_are_auto_answered() {
        let (_client, _events, mut sent, server) = loopback();
        // Known: workDoneProgress/create gets a null success.
        server
            .send(Message::Request(Request {
                jsonrpc: "2.0".to_owned(),
                id: json!("prog-1"),
                method: "window/workDoneProgress/create".to_owned(),
                params: json!({"token": "t"}),
            }))
            .await
            .unwrap();
        let Some(Message::Response(resp)) = sent.recv().await else {
            panic!("expected auto-reply");
        };
        assert_eq!(resp.id, json!("prog-1"));
        assert!(resp.error.is_none());

        // Unknown: MethodNotFound, id echoed.
        server
            .send(Message::Request(Request {
                jsonrpc: "2.0".to_owned(),
                id: json!(42),
                method: "workspace/weirdThing".to_owned(),
                params: Value::Null,
            }))
            .await
            .unwrap();
        let Some(Message::Response(resp)) = sent.recv().await else {
            panic!("expected auto-reply");
        };
        assert_eq!(resp.id, json!(42));
        assert_eq!(resp.error.unwrap().code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn request_after_transport_close_errors() {
        let (client, _events, sent, server) = loopback();
        drop(sent); // server side of outgoing gone
        drop(server);
        let err = client
            .request::<lsp_types::request::Shutdown>(())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Closed));
    }
}
