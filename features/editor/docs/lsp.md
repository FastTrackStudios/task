# LSP support (`editor-lsp`)

How the editor talks to language servers: a host-side LSP client that
turns the editor's transaction stream into incremental document sync
and turns `publishDiagnostics` back into byte-range data the view can
decorate. Diagnostics are the v1 payload; hover and completion ride
the same plumbing later (see the roadmap).

Reference: the LSP 3.17 specification ("Base Protocol", "Text
Document Synchronization", "Publish Diagnostics"). CM6's lint package
informed the diagnostics-as-decorations shape.

## Where it sits

```
┌────────────────────────── host app (desktop/native) ──────────────────────────┐
│                                                                               │
│  <Editor> (editor-view)                     editor-lsp                        │
│  ┌────────────────────┐   TransactionEvent  ┌──────────────────────────────┐  │
│  │ Signal<EditorState>│ ──────────────────► │ LspClient                    │  │
│  │                    │  (changes,          │  didOpen/didChange/didClose  │  │
│  │  decorations ◄─────┼── doc_before)       │  version counters            │  │
│  └────────────────────┘                     │  request/response router     │  │
│        ▲                                    └──────────────┬───────────────┘  │
│        │ DecoratedRange marks                              │ Transport        │
│  ┌─────┴──────────────┐   ServerMessage::Diagnostics       │ (Message pair)   │
│  │ DiagnosticsStore   │ ◄──────────────────────────────────┤                  │
│  │  version filter,   │                                    │                  │
│  │  byte-range resolve│                     stdio backend  │  websocket proxy │
│  └────────────────────┘                     (child process)│  (wasm, later)   │
└────────────────────────────────────────────────────────────┼──────────────────┘
                                                             ▼
                                                   language server (rust-analyzer, …)
```

Design rules:

- **The client runs host-side.** `editor-lsp` depends on tokio +
  `editor-state` only — no dioxus, no `editor-view`. The host owns
  the `LspClient` next to its editor signal and does the wiring. This
  keeps the view layer LSP-agnostic and lets the same client serve a
  future non-Dioxus view.
- **Bytes at the boundary.** Everything the host touches is byte
  offsets into the current `Doc`, same as decorations and selections.
  UTF-16 line/character positions (LSP's default and only universally
  implemented encoding) exist *only* inside `editor-lsp::pos`.
- **No tower-lsp.** That's a server framework; a client needs ~200
  lines of JSON-RPC (framing codec + id correlation), which we own
  and test directly.

## Crate layout

| Module | Job |
|---|---|
| `transport` | `Content-Length` framing codec (`encode` / `FrameDecoder`, pure, buffer-testable) + the `Transport` channel pair + the stdio child-process backend. |
| `pos` | Byte offset ↔ UTF-16 `Position` conversion against a `Doc`; `Changes` → incremental `TextDocumentContentChangeEvent` translation. The correctness-critical seam — exhaustively unit-tested (multi-byte UTF-8, surrogate pairs, clamping, round-trips). |
| `client` | `LspClient`: initialize/initialized + shutdown/exit lifecycle, didOpen/didChange/didClose with per-URI version counters, request/response correlation, `ServerMessage` push channel, auto-answers for server→client requests. |
| `diagnostics` | `PublishedDiagnostics` (raw) → `Diagnostic` (byte range + severity + message) resolution, `DiagnosticsStore` with stale-version filtering and local-edit mapping, `to_decorations` for the view. |

## The transport abstraction

A `Transport` is deliberately nothing but a pair of `Message`
channels:

```rust
pub struct Transport {
    pub outgoing: mpsc::Sender<Message>,   // client → server
    pub incoming: mpsc::Receiver<Message>, // server → client
}
```

`Transport::stdio(cmd, args, cwd)` backs the pair with a spawned
child process: a writer task frames `outgoing` messages onto the
child's stdin, a reader task feeds stdout through the `FrameDecoder`
into `incoming`, and the child is spawned `kill_on_drop` so dropping
the transport (or the client) reaps it.

That reduction is the wasm story: a browser host can't spawn
processes, so a **websocket proxy** (a tiny native sidecar or remote
service that owns the child process and relays framed messages over a
socket) backs the *same* channel pair via
`Transport::from_channels` — `LspClient` and everything above it is
unchanged. The loopback tests in `client.rs` already exercise this
seam by playing the server on the far side of raw channels.

## Wiring a host

The host feeds `TransactionEvent`s in and maps diagnostics out.
Sketch (host-side pseudocode; the async calls run on the host's tokio
runtime):

```rust
// Startup.
let transport = Transport::stdio("rust-analyzer", &[], Some(&project_root))?;
let (client, mut server_msgs) = LspClient::new(transport);
client.initialize(Some(root_uri)).await?;
client.did_open(uri.clone(), "rust", &doc).await?;
let mut store = DiagnosticsStore::new();

// Edit path — from the <Editor>'s on_transaction callback:
if event.is_edit() && !event.is_remote() {
    // Keep existing squiggles anchored while the server recomputes…
    store.map_through(&uri, &event.changes);
    // …and sync incrementally (bumps + returns the version).
    client.did_change(&uri, &event.changes, &event.doc_before).await?;
}

// Diagnostics path — a task draining the server channel:
while let Some(msg) = server_msgs.recv().await {
    if let ServerMessage::Diagnostics(published) = msg {
        // Stale publishes (older than the version we last sent, or
        // than the last accepted publish) are dropped inside apply.
        if store.apply(&published, client.version_of(&uri), &current_doc) {
            // Rebuild decorations: byte-range marks with severity
            // classes (cm-lsp-error / -warning / -info / -hint) and
            // the message in data-lsp-message.
            let decos = editor_lsp::to_decorations(store.get(&uri));
            // …hand `decos` to the editor's decoration source.
        }
    }
}
```

Notes on the seams:

- `did_change` takes `(changes, doc_before)` — exactly the fields a
  `TransactionEvent` carries — rather than the event type itself, so
  the crate never depends on `editor-view`.
- Selection-only transactions have empty `changes`; filter with
  `event.is_edit()`. Host-applied remote (CRDT) edits *do* need to
  reach the server — the `is_remote()` filter above assumes the CRDT
  host syncs those on its own path; a single-user IDE host would
  drop that check.
- `to_decorations` produces plain `DecoratedRange` marks, which the
  expensive-analysis pass of the decoration pipeline should supply
  (`DecoPhase::Full` — see `editor-state::decoration`).

## Position conversion (`pos`)

The one place the two coordinate systems meet. Editor positions are
byte offsets into UTF-8; LSP positions are `{line, character}` with
`character` in UTF-16 code units (an emoji is 4 bytes to us, 2
characters to the server). Documents are `\n`-only — the editor never
stores CRLF, so no line-ending ambiguity exists.

- Conversions use the rope's O(log n) line index and scan only the
  within-line prefix.
- Spec clamping: line past EOF → doc end; character past line end →
  before the `\n`; character splitting a surrogate pair → rounds down
  to the code point start. Byte offsets inside a multi-byte char
  floor to the char start.
- **Incremental didChange ordering:** every `Change` in a `Changes`
  is addressed against `doc_before` (simultaneous), but LSP content
  changes apply sequentially. Emitting the sorted, non-overlapping
  changes in *reverse document order* makes both views agree —
  applying a later-in-document edit never shifts earlier positions.
  A unit test cross-checks by simulating sequential application.

## Version tracking

Three version numbers interact:

1. **Sent version** — `LspClient` bumps a per-URI counter on every
   `did_change` and stamps it on the notification (didOpen = 1).
   Readable via `version_of(&uri)`.
2. **Published version** — servers supporting `versionSupport`
   (which we advertise) stamp `publishDiagnostics` with the version
   they analyzed. `None` for servers that don't.
3. **Store version** — the version of the last publish
   `DiagnosticsStore` accepted.

`DiagnosticsStore::apply` drops a publish when its version is older
than the sent version (computed against text the user has since
edited — resolving its UTF-16 ranges against the current doc would
misplace every squiggle) or older than the last accepted publish
(out-of-order delivery). Versionless publishes are accepted on faith,
per spec. Between an edit and the next fresh publish,
`map_through` shifts the stored byte ranges through the local
`Changes` (same `map_position` machinery decorations use) so
squiggles don't visually drift.

## Testing

- `pos` — 16 unit tests: multi-byte UTF-8 (2/3/4-byte), surrogate
  pairs, clamping, empty docs, trailing newlines, a full-document
  char-boundary round-trip, and content-change ordering with a
  sequential-application cross-check.
- `transport` — 8 codec tests: encode/decode round-trip, frames split
  across feeds, multiple frames per feed, header case/extras, error
  cases, untagged message discrimination.
- `client` — 7 async loopback tests playing the server over
  `Transport::from_channels`: initialize handshake, version
  bookkeeping and wire shapes, diagnostics/notification routing,
  server-request auto-answers, closed-transport errors.
- `diagnostics` — 8 tests: UTF-16→byte resolution, zero-width
  widening, stale/out-of-order filtering, map-through, decoration
  output.
- `tests/rust_analyzer.rs` — one `#[ignore]`d end-to-end test that
  spawns rust-analyzer from PATH on a throwaway cargo project with a
  type error and waits for real diagnostics
  (`cargo test -p editor-lsp -- --ignored`).

## Roadmap

- **Hover** — `textDocument/hover` is a plain
  `client.request::<HoverRequest>(…)` away; the position converts via
  `pos::byte_to_position`. The view already has hover popup
  infrastructure (`editor-state::hover::HoverSource` feeding the
  editor's hover popup) — an LSP hover source resolves the request
  and returns the markdown contents.
- **Completion** — same shape: `textDocument/completion` on the
  typing path, mapped into the view's existing completion menu.
  Needs debounce + `$/cancelRequest` support in the client (add a
  `cancel(id)` that sends the notification and drops the pending
  entry).
- **Websocket proxy transport** — the sidecar that lets wasm hosts
  join; `Transport::from_channels` is the ready seam.
- **Pull diagnostics** (`textDocument/diagnostic`, LSP 3.17) — for
  servers moving off the push model; slots in as an alternative
  filler for the same `DiagnosticsStore`.
- **Multiple servers / documents** — `LspClient` already tracks
  versions per URI; a thin registry keyed by language id covers
  multi-server hosts.
