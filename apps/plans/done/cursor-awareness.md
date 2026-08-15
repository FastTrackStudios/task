# Cursor Awareness — multi-peer cursor sync

Mirrors Logseq / Obsidian's "you can see what I'm doing" UX. Real-
time cursors from every connected peer rendered as colored bars
in the active page, with a small label chip showing the peer's
identity.

Built on top of Loro's two native awareness primitives — no
hand-rolled offset bookkeeping, no protocol additions beyond a
new vox method.

## Primitives we get from Loro

### 1. `EphemeralStore` (loro-internal::awareness)

Timestamped last-write-wins key-value store, **partial updates**
(only changed keys ship), built-in timeouts (stale peers expire).

API surface we'll use:
- `EphemeralStore::new(timeout_ms)`
- `store.set(key, value)` — local write
- `store.encode(key)` → `Vec<u8>` — payload for one key
- `store.encode_all()` → `Vec<u8>` — full sync
- `store.apply(&[u8])` — absorb a remote encoded payload
- `store.subscribe_local_updates(cb)` — fires whenever local state
  changes; cb yields the encoded bytes to forward to peers
- `store.subscribe(cb)` — receives `EphemeralStoreEvent` with
  added/updated/removed keys (used for render invalidation)
- `store.remove_outdated()` — manual purge of expired keys; call
  on a timer

Re-export plan: add to our crate shim
```rust
// crates/crdt or similar
pub use loro_internal::awareness::{EphemeralStore, EphemeralStoreEvent};
```

### 2. Stable `LoroText` cursors

`Block.content` is already a `LoroText` container in our schema.
We get auto-transforming cursors for free:

```rust
let text = block_map.get_text("content");
let cursor: loro::Cursor = text.get_cursor(byte_offset, side)?;
let bytes: Vec<u8> = cursor.encode();   // wire format
// peer side:
let cursor: loro::Cursor = loro::Cursor::decode(&bytes)?;
let pos: loro::PosQueryResult = doc.get_cursor_pos(&cursor);
let local_offset = pos.current.pos;  // transformed to peer's local view
```

This is the magic: if a remote peer inserts text **before** our
cursor position, `get_cursor_pos` returns the *new* offset that
preserves the semantic location. We don't track byte offsets
across the wire — we track stable cursors.

## Wire format

One `EphemeralStore` per `CrdtDoc` (per page / vault doc).

Key: `"cursor::<peer_uuid>"` — one slot per peer (single primary
cursor first; multi-cursor adds `::<n>` suffix later).

Value (encoded as a Loro `LoroValue::Map`):
```json
{
  "page_id":      "<uuid>",
  "block_id":     "<uuid>",
  "text_cursor":  <bytes — loro::Cursor::encode()>,
  "fallback_off": <int — used when text_cursor can't be decoded
                   on the peer (block missing locally)>,
  "color":        "#7c3aed",
  "name":         "Cody",
  "mode":         "normal" | "insert" | "visual",
  "anchor":       <int — anchor offset for visual selection,
                   omitted in normal/insert>
}
```

`text_cursor` carries the *stable* position. Peers decode +
`doc.get_cursor_pos()` to get their local byte offset.
`fallback_off` is the original numeric offset — used by peers
that don't have the source block (e.g., page not loaded yet)
so we can still place a "phantom" cursor at the best guess.

## Sync transport

New vox method on `WorkspaceSync` (server side):

```rust
#[vox::service]
pub trait WorkspaceSync {
    // existing methods …

    /// Subscribe to awareness updates for a doc. Server forwards
    /// every peer's encoded ephemeral payload as it arrives.
    async fn subscribe_awareness(
        &self,
        doc_id: DocId,
    ) -> Rx<AwarenessFrame>;

    /// Publish a local awareness update.
    async fn publish_awareness(
        &self,
        doc_id: DocId,
        payload: AwarenessFrame,
    );
}

pub struct AwarenessFrame {
    pub from_peer: Uuid,
    /// EphemeralStore-encoded bytes for one or more keys.
    pub bytes: Vec<u8>,
}
```

Server-side state: per `doc_id`, an `EphemeralStore` plus a
fan-out of `subscribe_awareness` Tx handles. On publish:
1. `store.apply(&frame.bytes)` — absorb so future joiners get
   the latest snapshot via `subscribe_awareness` on join.
2. Forward the frame to every other subscriber.

Client lifecycle (per `KnowledgeLive`):
1. On mount, subscribe to awareness for the current doc. On the
   first message OR after a short timer, request the server's
   full snapshot via a one-shot `encode_all`-style RPC.
2. Bridge local `EphemeralStore::subscribe_local_updates` →
   `publish_awareness`. Debounce 50 ms so j/k/l/h spam doesn't
   flood the channel.
3. Bridge incoming `AwarenessFrame::bytes` →
   `local_store.apply(&bytes)`.
4. Tick a `remove_outdated()` every 5s to purge disconnected
   peers (`EphemeralStore::new` timeout 30s).

## Local state shape

`PageBody` gains a `Signal<RemoteCursors>`:

```rust
pub struct RemoteCursors {
    /// peer_id → resolved cursor state for THIS page.
    pub by_peer: HashMap<Uuid, RemoteCursor>,
}

pub struct RemoteCursor {
    pub block_id: Uuid,
    pub offset: usize,   // resolved via doc.get_cursor_pos
    pub color: String,
    pub name: String,
    pub mode: VimMode,
    pub anchor: Option<usize>,
}
```

Populated by an effect that subscribes to the local
`EphemeralStore` and re-runs `doc.get_cursor_pos()` on the
stored stable cursor whenever the doc changes (local + remote
edits) — keeps remote cursors visually correct even as the
document drifts.

## Render

`CursorRow` (and the active-block renderer) gain a second pass
that overlays remote cursors per-character. Plain markup:

```rsx
// In CursorRow, after the local cursor:
for (peer_id, rc) in remote_cursors.iter().filter(|(_, rc)| rc.block_id == block.id) {
    span {
        class: "absolute pointer-events-none",
        style: "left: {col_to_px(rc.offset)}px; ...",
        // colored bar + small name chip on first character of line
        div { class: "h-[1.1em] w-[2px] bg-[{rc.color}]" }
        if first_char_of_line {
            div { class: "text-[10px] px-1 rounded-sm",
                  style: "background: {rc.color}; color: white;",
                  "{rc.name}"
            }
        }
    }
}
```

For monospace (our current default) `col_to_px(offset)` is just
`offset * ch_width_px` measured once on mount.

## Identity / color assignment

Per-session ephemeral peer id (UUID v4 generated on
`KnowledgeLive` mount). Color picked deterministically from the
peer id (`hsl(hash(peer_id) % 360, 70%, 55%)`). Name pulled from
the authenticated user context when available (we have
`architect-auth` in the stack), falls back to "peer-<8 hex>"
when anonymous.

## Phases

### Phase 1 — Local-only proof
- Re-export `EphemeralStore` from our crdt shim
- Write+read locally in PageBody (no wire)
- Render remote-cursor overlay using the local-only store
- Demoable by mocking entries
- Tests for `RemoteCursors` resolution against snapshot

### Phase 2 — Wire ✅ shipped
- `subscribe_awareness` + `publish_awareness` on `WorkspaceSync`
- Server: per-doc `EphemeralStore` + broadcast fan-out + snapshot
  on subscribe + server-side echo filter on `from_peer == peer_id`
- Client: dedicated sub/pub `WorkspaceSyncClient` sessions;
  inbound drain via `vox::channel::<AwarenessFrame>()` mirrors
  `run_sync_loop`; outbound bridges `subscribe_local_updates` →
  `publish_awareness`; janitor ticks `remove_outdated` + rebuilds
  resolved `RemoteCursor` snapshot for the renderer.

50ms debounce ✅ shipped (wasm only via `gloo-timers`):
- Outbound publisher coalesces with trailing-edge 50ms timer.
  `futures::select` over `out_rx.next()` and `TimeoutFuture` —
  while pending, keep the latest payload; on timeout fire one
  frame. Caps wire traffic at ~20 frames/sec regardless of
  typing speed. Native build no-ops to the un-debounced path
  (timer crate is wasm-only and the awareness loop only runs
  in the browser anyway).

Stable cursors ✅ shipped:
- `BlockRepoLoro::text_handle(block_id) -> Option<LoroText>`
  exposes the live container handle.
- Publish path: `text.get_cursor(offset.min(len_unicode), Middle)`
  → `Cursor::encode()` → `stable_cursor_bytes`. Empty when the
  block isn't materialized yet (fresh first render).
- Resolve path (janitor): `Cursor::decode` → `doc.get_cursor_pos`
  → `current.pos` overrides `fallback_offset`. Decode failure or
  unresolvable cursor falls back to the byte offset on the wire,
  so peers without the source block render gracefully.

Follow-ups not yet wired:
- 50 ms debounce on outgoing (today publishes on every effect).
- Honor `PosQueryResult.update` by re-encoding the refreshed
  cursor + re-publishing to avoid history-replay slowdown over
  long sessions.
- Visual selection (Phase 3): add `anchor_bytes` field, render
  half-line highlight between resolved anchor + head.

### Phase 3 — Visual selections (Visual mode) ✅ shipped
- `CursorState.anchor: Option<Cursor>` added; vim engine sets it
  on `EnterVisual`, clears on `EnterNormal`. Motion paths preserve
  the anchor while moving the primary.
- `CursorPayload` gained `anchor_block_id` / `anchor_fallback_offset`
  / `anchor_stable_bytes` — same stable-cursor encoding as the
  primary, scoped to the selection-start container.
- `RemoteCursor.anchor: Option<RemoteCursorAnchor>` carries the
  resolved (block_id, offset) for the renderer.
- `RemoteCursorOverlay` renders per-line highlight rectangles
  between anchor and head. Single-block (same-block both ends):
  rects between offsets. Cross-block: each block consults the
  new `BlockOrderIndex` context to decide its role — anchor
  block (offset → end), head block (start → offset), middle block
  (full block), or outside. The head bar + name chip render only
  in the head block.
- New `outliner::BlockOrderIndex` exposes DFS order + per-block
  content for the overlay.
- Tests: `vim::cursor::selection_anchor_lifecycle` covers the
  begin/clear lifecycle; existing 27 cursor/engine tests still pass.

### Phase 4 — Identity surface ✅ shipped
- `PresenceStrip` component renders one avatar per remote peer
  (deduped by `peer_id`), floats top-right of `/knowledge`,
  initial-letter on the peer's HSL color. `data-testid` per peer
  for tests. Tooltip = peer name. Disappears when solo.
- Click-to-follow: chips are `<button>`s that call `on_follow`
  with the peer's `page_id`, jumping the local route to that
  page. `CursorPayload` gained a `page_id: Option<Uuid>` field
  shipped on the wire so peers can navigate to each other even
  without having the source block loaded. Button is `disabled`
  when the peer published before page resolution.

Follow-up:
- "Follow to block": scroll into view + temporarily highlight
  the peer's block after navigating. Needs scroll-to-block
  primitive; out of scope for the awareness plan.

### Phase 5 — Mode + identity polish ✅ shipped
- `PeerMode` (Normal/Insert/Visual) added to `RemoteCursor`,
  `mode: String` wire field added to `CursorPayload`. PageBody
  reads `vim_engine.mode()` on every publish; janitor decodes
  via `PeerMode::from_wire`.
- Overlay caret glyph is mode-aware: thin caret (2px) in Insert,
  block cursor (1ch wide, semi-opaque) in Normal/Visual — same
  Logseq/vim affordance as the local caret.
- `PresenceStrip` tooltip shows `"<name> • <MODE>"` (and
  `"… — click to follow"` when a page_id is available).
- `AwarenessHub::anonymous()` generates `peer-<8 hex>` identities
  so the strip no longer shows "L L L" for every peer. Auth-
  driven names can wire in later by swapping the constructor in
  `KnowledgeLive`.

## Open questions for the next planning turn

- **One ephemeral store per doc** vs **one per route** vs **one
  per app**? Per-doc keeps cursor scope correct (cursor on page
  A shouldn't ship to subscribers of page B), but means more
  store instances. Per-doc preferred.
- **Encoding `text_cursor`**: Loro's `Cursor` has an `encode()`
  → `Vec<u8>`. Embed as base64 in the value JSON, or as a
  binary value via `LoroValue::Binary` (cleaner if supported).
- **History during disconnect**: when a peer reconnects, do we
  resync via `encode_all()` from the server, or rely on each
  peer to re-publish? Server `encode_all()` snapshot on subscribe
  is simpler.
- **Conflict on the active editor**: my own typing already
  drives my caret via the textarea; the remote cursor for myself
  is irrelevant. Filter `peer_id != self_peer_id` in render.
