//! Per-file collaborative editing — the client half of the vault ⇄
//! CRDT reconciliation (`features/vault/vault-collab` is the server
//! half; `docs/architecture/vault-crdt-reconciliation.md` is the
//! policy note).
//!
//! When the vault page opens a file it calls
//! [`VaultSync::open_collab`](vault_proto::VaultSync::open_collab),
//! which validates the path and returns the file's deterministic doc
//! id ([`vault_proto::collab_doc_id`]). [`CollabSession`] — mounted
//! keyed by that id — then drives the collaborative lifecycle:
//!
//! - `crdt::use_synced_doc_into(handles.doc, doc_id)` — an ephemeral
//!   local replica plus one sync session against the org's shared vox
//!   connection (delta reconnects by version vector);
//! - `crdt::use_presence_channel_into(handles.presence, doc_id,
//!   30_000)` — the per-file cursor channel;
//! - a **takeover** step: once the server's seeded history merges in
//!   (first doc revision), the session reconciles the editor buffer
//!   with the replica and flips [`CollabHandles::live`]. From then
//!   on the *server write-behind* owns persistence and the page
//!   pauses the sha autosave.
//!
//! **Signal ownership (the part that bit us):** every signal inside
//! [`CollabHandles`] is created at the *page* scope by
//! [`use_collab_handles`] and merely **driven** by the keyed
//! [`CollabSession`] child (the crdt slot/driver split —
//! `crdt::use_doc_slot` + `crdt::use_synced_doc_into`). The editor's
//! decoration source, `on_transaction` sink, and the page's
//! status/teardown effects all capture these handles; if they were
//! created inside the keyed child (as they once were), every remount
//! — file switch, reconnect-generation re-key, Live→Offline teardown
//! — dropped them while the still-mounted `Editor` kept reading them
//! from its keydown path, killing all input ("Copy value used in a
//! scope that is not a descendant of the owner"). Hoisting ownership
//! to the page scope is the fix dioxus prescribes for exactly this.
//!
//! Steady state is two one-way bridges meeting at the Loro doc:
//!
//! ```text
//! editor on_transaction ──(skip is_remote)──▶ changes_to_text_ops ─▶ LoroText
//! doc revision bump ──▶ read_text ─▶ remote_text_to_changes ─▶
//!     state.update(spec.changes(c).user_event("remote"))   // echo-guarded
//! ```
//!
//! If the sync session drops to `Offline` after going live, the page
//! tears the session down (unmount) and falls back to sha saves —
//! a *fresh* replica is opened on the next file open, so offline sha
//! writes can never double-apply against a buffered CRDT outbox.

use std::rc::Rc;

use dioxus::prelude::*;
use editor::editor_view;
use editor::markdown::VaultLookup;
use editor::{Doc, EditorState, TransactionEvent, TransactionSpec};
use uuid::Uuid;
use vault_proto::COLLAB_TEXT_CONTAINER;

use crate::document_session::DocumentSession;
use crate::vault_lookup::ClientVaultIndex;

/// Presence expiry for the per-file cursor channel — mirrors the
/// org-wide roster channel and the server registry's default.
const CURSOR_TIMEOUT_MS: i64 = 30_000;

/// Debounce between a cursor move and the presence publish. Caret
/// motion is per-keystroke; remote peers only need the settled spot.
const CURSOR_DEBOUNCE_MS: u64 = 200;

/// `Copy` bundle the vault page reads: the synced doc, the cursor
/// channel, and the takeover flag. **Every signal in here is owned by
/// the page scope** ([`use_collab_handles`]) — the keyed
/// [`CollabSession`] child only drives them — so the editor's
/// long-lived closures can capture the bundle safely across session
/// remounts (see the module docs).
#[derive(Clone, Copy, PartialEq)]
pub struct CollabHandles {
    pub doc: crdt::DocHandle,
    pub presence: crdt::Presence,
    /// `true` once the replica merged the server's seeded state and
    /// collab took over persistence from the sha autosave.
    pub live: Signal<bool>,
    /// This client's presence key (skipped when rendering peers).
    key: Signal<String>,
    /// Debounce generation for cursor publishes.
    cursor_seq: Signal<u64>,
}

impl CollabHandles {
    /// Collab is live right now: taken over AND the sync session is
    /// up. Reactive (subscribes to both signals).
    pub fn is_live(&self) -> bool {
        (self.live)() && self.doc.status() == crdt::SyncStatus::Live
    }

    /// Reset the per-file state (doc slot, presence slot, takeover
    /// flag) — the page calls this whenever it disarms a session
    /// (file switch, reconnect re-key, Live→Offline teardown), so a
    /// stale read through a captured handle only ever sees inert
    /// defaults. The client cursor key and debounce counter are
    /// deliberately page-lived.
    pub fn reset(&self) {
        self.doc.reset();
        self.presence.reset();
        let mut live = self.live;
        if *live.peek() {
            live.set(false);
        }
    }
}

/// Create the page-owned [`CollabHandles`]: empty crdt slots + the
/// takeover flag + this client's stable cursor identity. Call once in
/// the vault page (a scope that outlives the `Editor`); mount a keyed
/// [`CollabSession`] to drive the slots per file.
pub fn use_collab_handles() -> CollabHandles {
    CollabHandles {
        doc: crdt::use_doc_slot(),
        presence: crdt::use_presence_slot(),
        live: use_signal(|| false),
        key: use_signal(|| format!("cursor-{}", Uuid::new_v4())),
        cursor_seq: use_signal(|| 0u64),
    }
}

/// Headless component driving one file's collab session **into** the
/// page-owned [`CollabHandles`]. Mount keyed by `doc_id` (remount =
/// fresh replica; the drivers reset the slots on mount); reads the
/// page's [`DocumentSession`] from context. Owns no signals that
/// outlive it — only the session futures and the takeover effect die
/// with this scope.
#[component]
pub fn CollabSession(doc_id: Uuid, handles: CollabHandles) -> Element {
    let session = use_context::<DocumentSession>();
    crdt::use_synced_doc_into(handles.doc, doc_id);
    crdt::use_presence_channel_into(handles.presence, doc_id, CURSOR_TIMEOUT_MS);
    let handle = handles.doc;
    let live = handles.live;

    // Takeover + remote-apply. Runs on every doc revision (local
    // commits and remote imports both bump it).
    use_effect(move || {
        let rev = handle.revision(); // subscribe — the only reactive read
        let Some(doc) = handle.doc() else { return };
        if rev == 0 {
            // Nothing merged yet. The server factory always commits a
            // baseline (meta.flushed_sha), so the seeded backlog is
            // never empty — rev 0 strictly means "before first sync".
            return;
        }
        let remote = doc.loro().get_text(COLLAB_TEXT_CONTAINER).to_string();
        let mut live = live;

        if !*live.peek() {
            // ── Takeover ───────────────────────────────────────────
            // Reconcile editor buffer ⇄ replica exactly once, three-
            // way against the session's last committed text:
            //  - buffer == replica → nothing to do;
            //  - user hasn't typed since open → the replica (which
            //    includes peers' newer edits) wins, editor adopts it;
            //  - user typed during the handshake → fold the local
            //    delta (committed → buffer) into the replica; the
            //    commit re-runs this effect in live mode, which then
            //    settles the editor on the merged text.
            let buffer = session.state.peek().doc.to_string();
            if buffer != remote {
                let committed = session.committed_text();
                if buffer == committed {
                    apply_remote_to_editor(session.state, &remote);
                } else {
                    let base = Doc::from_str(&committed);
                    let changes = editor_crdt::remote_text_to_changes(base.rope(), &buffer);
                    let ops = editor_crdt::changes_to_text_ops(base.rope(), &changes);
                    let text = doc.loro().get_text(COLLAB_TEXT_CONTAINER);
                    apply_ops_clamped(&text, &ops);
                    doc.loro().commit();
                }
            }
            live.set(true);
            return;
        }

        // ── Steady state ───────────────────────────────────────────
        // After a local keystroke the replica already equals the
        // buffer (the on_transaction bridge ran first) → empty diff,
        // no-op. After a remote import the diff is the peers' edit.
        apply_remote_to_editor(session.state, &remote);
    });

    rsx! {}
}

/// Diff the editor buffer against the replica text and apply the
/// delta as a `"remote"`-tagged transaction. Host-driven `state.set`
/// never re-enters the `on_transaction` sink, and the tag is the
/// second-line echo guard (`event.is_remote()`).
fn apply_remote_to_editor(state: Signal<EditorState>, remote: &str) {
    let cur = state.peek().clone();
    let changes = editor_crdt::remote_text_to_changes(cur.doc.rope(), remote);
    if changes.is_empty() {
        return;
    }
    let mut state = state;
    state.set(cur.update(TransactionSpec::new().changes(changes).user_event("remote")));
}

/// Push a **full-document replacement** into the live replica: diff
/// the current replica text against `new_text`, apply the delta as
/// scalar ops, and commit. The [`CollabSession`] revision effect then
/// echoes the merged text back into the editor buffer
/// ([`apply_remote_to_editor`]) — so a host-side rewrite (the note
/// header's frontmatter splice) reaches BOTH the shared doc and the
/// editor without a host-driven `state.set` (which would never reach
/// peers, since it doesn't re-enter `on_transaction`). No-op when the
/// replica isn't attached yet.
pub fn push_full_text(c: &CollabHandles, new_text: &str) {
    let Some(doc) = c.doc.doc() else { return };
    let text = doc.loro().get_text(COLLAB_TEXT_CONTAINER);
    let base = Doc::from_str(&text.to_string());
    let changes = editor_crdt::remote_text_to_changes(base.rope(), new_text);
    if changes.is_empty() {
        return;
    }
    let ops = editor_crdt::changes_to_text_ops(base.rope(), &changes);
    apply_ops_clamped(&text, &ops);
    doc.loro().commit();
}

/// The vault page's `on_transaction` sink body: forward local edits
/// into the replica (echo-guarded) and publish the caret to the
/// file's presence channel (debounced).
pub fn on_editor_transaction(
    c: &CollabHandles,
    session: &DocumentSession,
    event: &TransactionEvent,
    who: &str,
) {
    if event.is_remote() {
        return; // echo guard: a remote application leaked an event
    }
    if !*c.live.peek() {
        // Pre-takeover keystrokes stay editor-only; the takeover's
        // three-way fold picks them up.
        return;
    }
    let Some(doc) = c.doc.doc() else { return };

    if event.is_edit() {
        let text = doc.loro().get_text(COLLAB_TEXT_CONTAINER);
        let replica = text.to_string();
        if replica == event.doc_before.to_string() {
            // Fast path: replica is exactly the pre-change doc — the
            // event's ops apply verbatim (scalar positions).
            let ops = editor_crdt::changes_to_text_ops(event.doc_before.rope(), &event.changes);
            apply_ops_clamped(&text, &ops);
        } else {
            // Streams interleaved mid-frame (remote import between
            // the keystroke and this sink) — fall back to a plain
            // diff onto the post-change text. Not minimal, always
            // convergent.
            let after = event.doc_after.to_string();
            let base = Doc::from_str(&replica);
            let changes = editor_crdt::remote_text_to_changes(base.rope(), &after);
            let ops = editor_crdt::changes_to_text_ops(base.rope(), &changes);
            apply_ops_clamped(&text, &ops);
        }
        doc.loro().commit();
    }

    publish_cursor(c, session, event, who);
}

/// Debounced presence publish: `{name, anchor_c, head_c}` where the
/// `_c` values are **encoded Loro stable cursors** minted against the
/// shared replica, plus legacy `{anchor, head}` scalar ints for
/// mixed-version peers. Fires for selection-only transactions too —
/// that's the point.
///
/// Stable cursors are the fix for "their caret walks backwards
/// through my word": a raw scalar published at time T stays fixed
/// while MY local inserts shift the text under it, so between the
/// peer's debounced republishes their caret visually drifts. A Loro
/// cursor is anchored to the character identity instead; every
/// viewer resolves it against their *own* current doc state
/// (`get_cursor_pos`), so local edits move remote carets correctly
/// in real time.
fn publish_cursor(
    c: &CollabHandles,
    session: &DocumentSession,
    event: &TransactionEvent,
    who: &str,
) {
    let primary = session.state.peek().selection.primary();
    let rope = event.doc_after.rope();
    let clamp = |byte: usize| rope.byte_to_char(byte.min(rope.len_bytes()));
    let (anchor, head) = (clamp(primary.anchor), clamp(primary.head));

    let seq = c.cursor_seq.peek().wrapping_add(1);
    {
        let mut cursor_seq = c.cursor_seq;
        cursor_seq.set(seq);
    }
    let c = *c;
    let name = who.to_string();
    spawn(async move {
        architect::sleep(architect::Duration::from_millis(CURSOR_DEBOUNCE_MS)).await;
        if *c.cursor_seq.peek() != seq {
            return; // a newer cursor superseded this one
        }
        let key = c.key.peek().clone();
        let mut entries = vec![
            ("name".to_string(), crdt::loro::LoroValue::from(name)),
            (
                "anchor".to_string(),
                crdt::loro::LoroValue::from(anchor as i64),
            ),
            ("head".to_string(), crdt::loro::LoroValue::from(head as i64)),
        ];
        // Mint stable cursors against the replica at publish time
        // (post-debounce — the replica has absorbed the edit batch).
        // Side::Left binds the caret to the char it follows, so a
        // collision-insert exactly at a peer's caret lands AFTER it.
        if let Some(doc) = c.doc.doc() {
            let text = doc.loro().get_text(COLLAB_TEXT_CONTAINER);
            let len = text.len_unicode();
            let mut mint = |label: &str, pos: usize| {
                if let Some(cur) = text.get_cursor(pos.min(len), crdt::loro::cursor::Side::Left) {
                    entries.push((
                        label.to_string(),
                        crdt::loro::LoroValue::Binary(cur.encode().into()),
                    ));
                }
            };
            mint("anchor_c", anchor);
            mint("head_c", head);
        }
        let value = crdt::loro::LoroValue::Map(entries.into());
        c.presence.set(&key, value);
    });
}

/// Apply scalar text ops to the replica's root text, clamping to the
/// current length — positions can drift when streams interleave; the
/// conflict policy's "may interleave, never loses whole edits".
fn apply_ops_clamped(text: &crdt::loro::LoroText, ops: &[editor_crdt::TextOp]) {
    for op in ops {
        let len = text.len_unicode();
        let result = match op {
            editor_crdt::TextOp::Insert { pos, text: t } => {
                text.insert((*pos as usize).min(len), t)
            }
            editor_crdt::TextOp::Delete { pos, len: n } => {
                let from = (*pos as usize).min(len);
                let n = (*n as usize).min(len - from);
                if n == 0 {
                    continue;
                }
                text.delete(from, n)
            }
        };
        if let Err(e) = result {
            tracing::warn!("collab: text op failed: {e}");
        }
    }
}

// ── Remote cursors → decorations ──────────────────────────────────

/// The vault page's decoration source: the existing vault pass
/// (live-preview with wikilink resolution + bracket match) plus
/// remote presence cursors/selections layered on top, plus every
/// registered widget's decoration pass (`widget_passes` — from
/// `WidgetRegistry::decoration_passes`, e.g. the section tabs). Create
/// once (`use_hook`) — captures the signals, equality is `Rc` identity;
/// the widget passes are self-gating plain fns, stable for the app's
/// life, so baking them in here keeps that contract.
pub fn collab_decoration_source(
    lookup: Signal<Option<Rc<ClientVaultIndex>>>,
    collab: Signal<Option<CollabHandles>>,
    widget_passes: Vec<task_widgets::DecorationPass>,
) -> editor_view::DecorationSource {
    editor_view::DecorationSource::new(move |state| {
        let mut out = {
            let guard = lookup.read();
            let resolver = guard.as_ref().map(|rc| rc.as_ref() as &dyn VaultLookup);
            let mut v = editor::markdown::live_preview_with(state, resolver);
            v.extend(editor::bracket_match::bracket_match(state));
            v
        };
        if let Some(c) = &*collab.read() {
            out.extend(remote_cursor_decorations(state, c));
        }
        for pass in &widget_passes {
            out.extend(pass(state));
        }
        out
    })
}

/// One `Mark` per remote selection + one caret `Widget` (with the
/// peer's name) per remote head.
///
/// Positions resolve from the peer's published **stable cursors**
/// against OUR replica (`get_cursor_pos`) — so our own in-flight
/// typing shifts remote carets correctly instead of leaving them
/// pinned to a stale scalar until the peer republishes. Falls back
/// to the legacy raw scalars for mixed-version peers. Scalar→byte
/// through the current rope, clamped — a peer's position can
/// momentarily outrun our replica view.
fn remote_cursor_decorations(
    state: &EditorState,
    c: &CollabHandles,
) -> Vec<editor::DecoratedRange> {
    // `states()` reads the presence revision signal: the editor
    // re-renders (and this pass re-runs) on every cursor update.
    let states = c.presence.states();
    let own = c.key.read().clone();
    let doc = c.doc.doc();
    let rope = state.doc.rope();
    let max_char = rope.len_chars();
    let mut out = Vec::new();

    for (key, value) in states {
        if key == own || !key.starts_with("cursor-") {
            continue;
        }
        let crdt::loro::LoroValue::Map(map) = value else {
            continue;
        };
        let read_i64 = |k: &str| match map.get(k) {
            Some(crdt::loro::LoroValue::I64(v)) => Some(*v),
            _ => None,
        };
        // Stable cursor first (resolved against our replica), legacy
        // scalar as the fallback.
        let resolve = |k_cursor: &str, k_legacy: &str| -> Option<usize> {
            if let (Some(crdt::loro::LoroValue::Binary(bytes)), Some(doc)) =
                (map.get(k_cursor), doc.as_ref())
            {
                if let Ok(cur) = crdt::loro::cursor::Cursor::decode(bytes) {
                    if let Ok(q) = doc.loro().get_cursor_pos(&cur) {
                        return Some(q.current.pos);
                    }
                }
            }
            read_i64(k_legacy).map(|v| usize::try_from(v.max(0)).unwrap_or(0))
        };
        let (Some(anchor), Some(head)) = (resolve("anchor_c", "anchor"), resolve("head_c", "head"))
        else {
            continue;
        };
        let name = match map.get("name") {
            Some(crdt::loro::LoroValue::String(s)) => s.to_string(),
            _ => "peer".to_string(),
        };
        let to_byte = |scalar: usize| {
            let ch = scalar.min(max_char);
            rope.char_to_byte(ch)
        };
        let (a, h) = (to_byte(anchor), to_byte(head));
        let hue = hue_of(&key);

        if a != h {
            let (from, to) = (a.min(h), a.max(h));
            out.push(editor::DecoratedRange::mark_with_attrs(
                from..to,
                "collab-remote-selection",
                vec![(
                    "style".to_string(),
                    format!("background-color: hsla({hue}, 80%, 60%, 0.22);"),
                )],
            ));
        }
        out.push(editor::DecoratedRange::widget(
            h,
            format!(
                "<span class=\"collab-caret\" style=\"border-color: hsl({hue}, 80%, 55%);\">\
                 <span class=\"collab-caret-label\" style=\"background: hsl({hue}, 80%, 40%);\">{}</span>\
                 </span>",
                escape_html(&name)
            ),
        ));
    }
    out
}

/// Stable per-peer hue from the presence key (FNV-1a → 0..360).
/// Every client derives the same color for the same peer.
fn hue_of(key: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in key.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h % 360
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Styles for the remote cursor decorations. Injected by the vault
/// page next to `editor::EDITOR_STYLE`. Colors are per-peer identity
/// hues (computed inline), not theme tokens — by design, like
/// avatars.
pub const COLLAB_STYLE: &str = "
.collab-remote-selection { border-radius: 2px; }
.collab-caret {
  position: relative;
  border-left: 2px solid;
  margin-left: -1px;
  pointer-events: none;
}
.collab-caret-label {
  position: absolute;
  top: -1.15em;
  left: -2px;
  font-size: 0.625rem;
  line-height: 1.2;
  padding: 0 4px;
  border-radius: 3px 3px 3px 0;
  color: white;
  white-space: nowrap;
  user-select: none;
  pointer-events: none;
  z-index: 30;
}
";

// ── RPC ───────────────────────────────────────────────────────────

/// Register the open file for collaboration and get its doc id (+
/// the server's current sha). An `Err` simply means "no collab" —
/// the page stays in plain sha mode.
pub(crate) async fn open_collab(
    slug: String,
    path: String,
) -> Result<vault_proto::CollabAck, String> {
    let client = crate::vox_clients::vault_client(&slug).await?;
    #[cfg(target_arch = "wasm32")]
    {
        client
            .open_collab(crate::document_session::VAULT_ID.to_owned(), path)
            .await
            .map_err(|e| format!("open_collab: {e:?}"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (client, path);
        Err("native client not wired yet".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::Changes;

    /// Adapter round-trip through a REAL LoroDoc: editor `Changes`
    /// (byte offsets) → `editor_crdt::changes_to_text_ops` (scalar)
    /// → applied to a `LoroText` — must reproduce `Changes::apply`.
    #[test]
    fn adapter_round_trips_through_loro() {
        let cases: Vec<(&str, Changes)> = vec![
            ("hello", Changes::insert(5, " world")),
            ("hello world", Changes::delete(5..11)),
            ("hello world", Changes::replace(6..11, "rust")),
            // Unicode: byte offsets vs scalar positions.
            ("a🦀b", Changes::insert(5, "X")),
            ("日本語テスト", Changes::delete(0..6)),
            ("", Changes::insert(0, "fresh")),
        ];
        for (text, changes) in cases {
            let doc_before = Doc::from_str(text);
            let expected = changes.apply(&doc_before).to_string();

            let loro = crdt::loro::LoroDoc::new();
            let t = loro.get_text(COLLAB_TEXT_CONTAINER);
            t.insert(0, text).unwrap();
            let ops = editor_crdt::changes_to_text_ops(doc_before.rope(), &changes);
            apply_ops_clamped(&t, &ops);
            assert_eq!(t.to_string(), expected, "diverged for {text:?}");
        }
    }

    /// Full remote cycle through a real LoroDoc: a remote replica's
    /// text lands in the editor via `remote_text_to_changes`, and the
    /// resulting state matches the replica exactly.
    #[test]
    fn remote_apply_round_trips_through_loro() {
        let loro = crdt::loro::LoroDoc::new();
        let t = loro.get_text(COLLAB_TEXT_CONTAINER);
        t.insert(0, "shared 🦀 document").unwrap();

        let editor_state = EditorState::new("shared 🦀 doc");
        let remote = t.to_string();
        let changes = editor_crdt::remote_text_to_changes(editor_state.doc.rope(), &remote);
        let next =
            editor_state.update(TransactionSpec::new().changes(changes).user_event("remote"));
        assert_eq!(next.doc.to_string(), remote);
    }

    #[test]
    fn clamped_ops_tolerate_out_of_range() {
        let loro = crdt::loro::LoroDoc::new();
        let t = loro.get_text(COLLAB_TEXT_CONTAINER);
        t.insert(0, "ab").unwrap();
        apply_ops_clamped(
            &t,
            &[
                editor_crdt::TextOp::Insert {
                    pos: 99,
                    text: "!".into(),
                },
                editor_crdt::TextOp::Delete { pos: 99, len: 5 },
            ],
        );
        assert_eq!(t.to_string(), "ab!");
    }

    #[test]
    fn hue_is_stable_and_bounded() {
        assert_eq!(hue_of("cursor-x"), hue_of("cursor-x"));
        assert!(hue_of("cursor-x") < 360);
        assert!(hue_of("cursor-y") < 360);
    }

    #[test]
    fn html_escapes_peer_names() {
        assert_eq!(escape_html("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
    }
}
