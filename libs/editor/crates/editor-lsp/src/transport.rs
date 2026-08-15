//! Framed JSON-RPC transport — the wire layer under
//! [`crate::client::LspClient`].
//!
//! LSP is JSON-RPC 2.0 with HTTP-style `Content-Length` framing
//! (see the "Base Protocol" section of the LSP spec). We write the
//! client ourselves rather than pull in tower-lsp — that crate is a
//! *server* framework and drags in far more than a client needs.
//!
//! Two layers live here:
//!
//! - **Codec** — [`encode`] / [`FrameDecoder`]: pure functions that
//!   turn a [`Message`] into `Content-Length`-framed bytes and back.
//!   No I/O, so the framing rules are unit-testable from buffers.
//! - **Transport** — a [`Transport`] is nothing but a pair of
//!   [`Message`] channels (client→server, server→client). That *is*
//!   the transport abstraction: [`Transport::stdio`] backs the pair
//!   with a spawned child process today; a wasm host later backs the
//!   same pair with a websocket proxy via
//!   [`Transport::from_channels`] without the client changing.
//!
//! Request/response correlation (matching a `Response.id` back to
//! the caller that sent the `Request`) is the client's job — the
//! transport only moves framed messages.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

/// The `jsonrpc` field every message carries. Kept as a serde
/// default so hand-built test messages don't have to spell it.
fn jsonrpc_version() -> String {
    "2.0".to_owned()
}

/// A client→server or server→client call that expects a response.
///
/// `id` stays a raw [`Value`] on the wire: JSON-RPC allows string
/// *or* number ids, and server-initiated requests (e.g.
/// `window/workDoneProgress/create`) pick their own — we must echo
/// whatever we got. The client generates numeric ids for its own
/// requests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

/// The error object of a failed [`Response`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC `MethodNotFound` — sent back for server→client requests
/// we don't implement, so the server isn't left waiting forever.
pub const METHOD_NOT_FOUND: i64 = -32601;

/// The reply to a [`Request`], correlated by `id`.
///
/// Note `result: None` and `"result": null` both deserialize to
/// `None` (serde's `Option<Value>` collapses them) — callers treat a
/// response with no `error` as success with `Value::Null` result,
/// which matches how `shutdown` and friends respond.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

/// A fire-and-forget message (no `id`, no response) — `didOpen`,
/// `didChange`, `publishDiagnostics`, …
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

/// Any JSON-RPC message. Untagged: the discriminator is field
/// presence — a `Request` has `id` *and* `method`, a `Response` has
/// `id` but no `method`, a `Notification` has `method` but no `id`.
/// Variant order matters for serde's first-match-wins untagged
/// deserialization: `Request` must be tried before the other two.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Request(Request),
    Response(Response),
    Notification(Notification),
}

/// Framing/decoding failures. A decode error is fatal for the
/// stream — once framing is lost there is no resync point.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("header block is not valid UTF-8")]
    HeaderNotUtf8,
    #[error("missing Content-Length header")]
    MissingContentLength,
    #[error("invalid Content-Length value: {0}")]
    InvalidContentLength(String),
    #[error("message body is not valid JSON-RPC: {0}")]
    Body(#[from] serde_json::Error),
}

/// Encode one message as `Content-Length: N\r\n\r\n<body>` bytes.
#[must_use]
pub fn encode(msg: &Message) -> Vec<u8> {
    let body = serde_json::to_vec(msg).expect("JSON-RPC message serializes");
    let len = body.len();
    let mut out = Vec::with_capacity(len + 32);
    out.extend_from_slice(format!("Content-Length: {len}\r\n\r\n").as_bytes());
    out.extend_from_slice(&body);
    out
}

/// Incremental decoder: feed it raw bytes as they arrive, pull
/// complete messages out. Handles messages split across reads and
/// multiple messages per read. Ignores headers other than
/// `Content-Length` (the spec's `Content-Type` is always UTF-8 in
/// practice).
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append newly-read bytes to the internal buffer.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Try to decode the next complete message. `Ok(None)` means
    /// "need more bytes"; call again after the next [`feed`].
    ///
    /// [`feed`]: FrameDecoder::feed
    pub fn try_next(&mut self) -> Result<Option<Message>, DecodeError> {
        let Some(header_end) = find_subslice(&self.buf, b"\r\n\r\n") else {
            return Ok(None);
        };
        let header =
            std::str::from_utf8(&self.buf[..header_end]).map_err(|_| DecodeError::HeaderNotUtf8)?;
        let mut content_length = None;
        for line in header.split("\r\n") {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            if name.trim().eq_ignore_ascii_case("content-length") {
                let value = value.trim();
                content_length = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| DecodeError::InvalidContentLength(value.to_owned()))?,
                );
            }
        }
        let len = content_length.ok_or(DecodeError::MissingContentLength)?;
        let body_start = header_end + 4;
        if self.buf.len() < body_start + len {
            return Ok(None);
        }
        let msg = serde_json::from_slice(&self.buf[body_start..body_start + len])?;
        self.buf.drain(..body_start + len);
        Ok(Some(msg))
    }
}

/// First index of `needle` in `haystack`, byte-wise.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// A live connection to a language server: a pair of [`Message`]
/// channels. This channel pair *is* the transport abstraction —
/// every backend (stdio child process now, websocket proxy for wasm
/// hosts later) reduces to "something that consumes `outgoing` and
/// produces `incoming`".
pub struct Transport {
    /// Client→server messages. Cloneable — the client keeps one copy
    /// for calls and hands one to its router for auto-replies.
    pub outgoing: mpsc::Sender<Message>,
    /// Server→client messages. Closed = server gone.
    pub incoming: mpsc::Receiver<Message>,
}

impl Transport {
    /// Wrap an existing channel pair. This is the seam for
    /// non-stdio backends (a wasm websocket proxy) and for tests
    /// that play the server in-process.
    #[must_use]
    pub fn from_channels(
        outgoing: mpsc::Sender<Message>,
        incoming: mpsc::Receiver<Message>,
    ) -> Self {
        Self { outgoing, incoming }
    }

    /// Spawn `cmd args…` and speak framed JSON-RPC over its
    /// stdin/stdout. Two tokio tasks pump the pipes:
    ///
    /// - *writer*: drains `outgoing`, frames with [`encode`], writes
    ///   to the child's stdin.
    /// - *reader*: reads stdout chunks through a [`FrameDecoder`],
    ///   forwards decoded messages to `incoming`. Owns the child
    ///   handle (spawned with `kill_on_drop`), so when stdout hits
    ///   EOF — or the `Transport` is dropped and the channel closes —
    ///   the child is reaped.
    ///
    /// stderr is inherited: language servers log there and hiding it
    /// makes failures undiagnosable.
    ///
    /// Must be called from within a tokio runtime.
    pub fn stdio(cmd: &str, args: &[&str], cwd: Option<&std::path::Path>) -> std::io::Result<Self> {
        let mut command = tokio::process::Command::new(cmd);
        command
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        let mut child = command.spawn()?;
        let mut stdin = child.stdin.take().expect("stdin piped");
        let mut stdout = child.stdout.take().expect("stdout piped");

        let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<Message>(64);
        let (incoming_tx, incoming_rx) = mpsc::channel::<Message>(64);

        // Writer: outgoing channel -> framed bytes -> child stdin.
        tokio::spawn(async move {
            while let Some(msg) = outgoing_rx.recv().await {
                if stdin.write_all(&encode(&msg)).await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
            // Channel closed (client dropped) or pipe broke — closing
            // stdin is the polite "no more input" signal.
        });

        // Reader: child stdout -> FrameDecoder -> incoming channel.
        // Owns `child` so the process is killed when this task ends.
        tokio::spawn(async move {
            let _child = child;
            let mut decoder = FrameDecoder::new();
            let mut chunk = [0u8; 8 * 1024];
            loop {
                let n = match stdout.read(&mut chunk).await {
                    Ok(0) | Err(_) => break, // EOF or pipe error
                    Ok(n) => n,
                };
                decoder.feed(&chunk[..n]);
                loop {
                    match decoder.try_next() {
                        Ok(Some(msg)) => {
                            if incoming_tx.send(msg).await.is_err() {
                                return; // client hung up
                            }
                        }
                        Ok(None) => break, // need more bytes
                        Err(_) => return,  // framing lost — unrecoverable
                    }
                }
            }
        });

        Ok(Self {
            outgoing: outgoing_tx,
            incoming: incoming_rx,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn notif(method: &str, params: Value) -> Message {
        Message::Notification(Notification {
            jsonrpc: jsonrpc_version(),
            method: method.to_owned(),
            params,
        })
    }

    #[test]
    fn encode_produces_content_length_frame() {
        let msg = notif("initialized", json!({}));
        let bytes = encode(&msg);
        let text = String::from_utf8(bytes).unwrap();
        let (header, body) = text.split_once("\r\n\r\n").unwrap();
        assert_eq!(header, format!("Content-Length: {}", body.len()));
        assert!(body.contains("\"initialized\""));
    }

    #[test]
    fn decode_roundtrips_encode() {
        let msg = Message::Request(Request {
            jsonrpc: jsonrpc_version(),
            id: json!(7),
            method: "initialize".into(),
            params: json!({"processId": 42}),
        });
        let mut dec = FrameDecoder::new();
        dec.feed(&encode(&msg));
        assert_eq!(dec.try_next().unwrap(), Some(msg));
        assert_eq!(dec.try_next().unwrap(), None);
    }

    #[test]
    fn decode_handles_message_split_across_feeds() {
        let bytes = encode(&notif("a/b", json!([1, 2])));
        let mut dec = FrameDecoder::new();
        // Split in the middle of the header, then mid-body.
        dec.feed(&bytes[..8]);
        assert!(dec.try_next().unwrap().is_none());
        dec.feed(&bytes[8..bytes.len() - 3]);
        assert!(dec.try_next().unwrap().is_none());
        dec.feed(&bytes[bytes.len() - 3..]);
        assert_eq!(dec.try_next().unwrap(), Some(notif("a/b", json!([1, 2]))));
    }

    #[test]
    fn decode_handles_two_messages_in_one_feed() {
        let mut bytes = encode(&notif("one", Value::Null));
        bytes.extend_from_slice(&encode(&notif("two", Value::Null)));
        let mut dec = FrameDecoder::new();
        dec.feed(&bytes);
        assert_eq!(dec.try_next().unwrap(), Some(notif("one", Value::Null)));
        assert_eq!(dec.try_next().unwrap(), Some(notif("two", Value::Null)));
        assert_eq!(dec.try_next().unwrap(), None);
    }

    #[test]
    fn decode_ignores_extra_headers_and_header_case() {
        let body = serde_json::to_vec(&notif("x", Value::Null)).unwrap();
        let mut bytes = format!(
            "content-length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n",
            body.len()
        )
        .into_bytes();
        bytes.extend_from_slice(&body);
        let mut dec = FrameDecoder::new();
        dec.feed(&bytes);
        assert_eq!(dec.try_next().unwrap(), Some(notif("x", Value::Null)));
    }

    #[test]
    fn decode_missing_content_length_is_an_error() {
        let mut dec = FrameDecoder::new();
        dec.feed(b"Content-Type: text/plain\r\n\r\n{}");
        assert!(matches!(
            dec.try_next(),
            Err(DecodeError::MissingContentLength)
        ));
    }

    #[test]
    fn decode_invalid_content_length_is_an_error() {
        let mut dec = FrameDecoder::new();
        dec.feed(b"Content-Length: banana\r\n\r\n{}");
        assert!(matches!(
            dec.try_next(),
            Err(DecodeError::InvalidContentLength(_))
        ));
    }

    #[test]
    fn untagged_message_discrimination() {
        // Request: id + method.
        let req: Message =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"m","params":{}}"#).unwrap();
        assert!(matches!(req, Message::Request(_)));
        // Response: id, no method.
        let resp: Message =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#).unwrap();
        assert!(matches!(resp, Message::Response(_)));
        // Error response.
        let err: Message = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}}"#,
        )
        .unwrap();
        let Message::Response(err) = err else {
            panic!("expected response");
        };
        assert_eq!(err.error.unwrap().code, METHOD_NOT_FOUND);
        // Notification: method, no id.
        let n: Message =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"m","params":[]}"#).unwrap();
        assert!(matches!(n, Message::Notification(_)));
    }
}
