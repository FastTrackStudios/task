//! Per-file collaborative editing — the client half of the vault ⇄
//! CRDT reconciliation (`features/vault/vault-collab` is the server
//! half; `docs/architecture/vault-crdt-reconciliation.md` is the
//! policy note; `docs/spec/offline.md` the rules).
//!
//! When the vault page opens a file it mounts a [`CollabSession`] keyed
//! by the file's doc id ([`vault_proto::collab_doc_id`], derived on the
//! device — no round trip needed to know it). The session drives:
//!
//! - [`crate::doc_replica::use_replica_into`] — the file's document as a
//!   **replica persisted on the device**, synced with the server's
//!   document whenever there is a connection, and kept (not torn down)
//!   when there isn't: edits go into the replica and up when the next
//!   session attaches (`offline.doc.replica`);
//! - `crdt::use_presence_channel_into(handles.presence, doc_id, 30_000)`
//!   — the per-file cursor channel;
//! - a **takeover** step: once the replica holds a version worth
//!   editing (stored on this device before, or the server's merged in),
//!   the session reconciles the editor buffer with it and flips
//!   [`CollabHandles::live`]. From then on the replica owns persistence:
//!   the page's sha autosave and explicit saves stay off for as long as
//!   the session is mounted — online or not — so a buffer the replica
//!   already carries can never also be written as a whole file (the
//!   double-apply the old Live→Offline teardown existed to prevent).
//!
//! **The document is `body` + `props`** (`offline.doc.shape`), but the
//! note editor's buffer is the whole file — frontmatter included — so
//! the Properties panel, the header and every widget keep reading one
//! text. The bridges translate through the shared codec (`vault-doc`):
//!
//! ```text
//! editor on_transaction ──(skip is_remote / mid-edit)──▶
//!     vault_doc::merge_text(doc, pushed, buffer)   // props by key, body by char
//! doc revision bump ──▶ vault_doc::render(doc, Some(buffer)) ─▶
//!     state.update(spec.changes(c).user_event("remote"))   // echo-guarded
//! ```
//!
//! `pushed` is the buffer as the document last reflected it; a buffer
//! whose frontmatter is half typed ([`vault_doc::is_mid_edit`]) is held
//! back until it parses, so a key being typed is never read as "every
//! property deleted".
//!
//! **Signal ownership (the part that bit us):** every signal inside
//! [`CollabHandles`] is created at the *page* scope by
//! [`use_collab_handles`] and merely **driven** by the keyed
//! [`CollabSession`] child. The editor's decoration source,
//! `on_transaction` sink, and the page's status effects all capture these
//! handles; if they were created inside the keyed child, every remount
//! dropped them while the still-mounted `Editor` kept reading them from
//! its keydown path ("Copy value used in a scope that is not a
//! descendant of the owner"). Hoisting ownership to the page scope is the
//! fix dioxus prescribes for exactly this.

use std::rc::Rc;

use dioxus::prelude::*;
use editor::editor_view;
use editor::markdown::VaultLookup;
use editor::{EditorState, TransactionEvent, TransactionSpec};
use uuid::Uuid;

use crate::doc_replica::ReplicaHandle;
use crate::document_session::DocumentSession;
use crate::vault_lookup::ClientVaultIndex;

/// Presence expiry for the per-file cursor channel — mirrors the
/// org-wide roster channel and the server registry's default.
const CURSOR_TIMEOUT_MS: i64 = 30_000;

/// Debounce between a cursor move and the presence publish. Caret
/// motion is per-keystroke; remote peers only need the settled spot.
const CURSOR_DEBOUNCE_MS: u64 = 200;

/// `Copy` bundle the vault page reads: the replica, the cursor channel,
/// and the takeover flag. **Every signal in here is owned by the page
/// scope** ([`use_collab_handles`]) — the keyed [`CollabSession`] child
/// only drives them.
#[derive(Clone, Copy, PartialEq)]
pub struct CollabHandles {
    pub doc: ReplicaHandle,
    pub presence: crdt::Presence,
    /// `true` once the buffer was reconciled with the replica and the
    /// replica took over persistence from the sha autosave.
    pub live: Signal<bool>,
    /// The buffer as the document last reflected it — the base the next
    /// local edit merges against.
    pushed: Signal<String>,
    /// This client's presence key (skipped when rendering peers).
    key: Signal<String>,
    /// Debounce generation for cursor publishes.
    cursor_seq: Signal<u64>,
}

impl CollabHandles {
    /// The replica owns this buffer's persistence (taken over). Stays
    /// true offline — that is the point. Reactive.
    pub fn is_live(&self) -> bool {
        (self.live)()
    }

    /// Connected to the server's document right now. Reactive.
    pub fn is_connected(&self) -> bool {
        self.doc.status() == crdt::SyncStatus::Live
    }

    /// Reset the per-file state (replica slot, presence slot, takeover
    /// flag) — the page calls this whenever it disarms a session (file
    /// switch, write mode change), so a stale read through a captured
    /// handle only ever sees inert defaults.
    pub fn reset(&self) {
        self.doc.reset();
        self.presence.reset();
        let mut live = self.live;
        if *live.peek() {
            live.set(false);
        }
    }

    /// The status-line word for this session.
    pub fn label(&self) -> &'static str {
        if self.doc.deleted() {
            "deleted elsewhere"
        } else if self.doc.refused().is_some() {
            "not shared — kept on this device"
        } else if !self.is_live() {
            "connecting…"
        } else if self.doc.unsynced() {
            "Not synced"
        } else if self.is_connected() {
            "live"
        } else {
            "offline — saved on this device"
        }
    }
}

/// Create the page-owned [`CollabHandles`]. Call once in the vault page
/// (a scope that outlives the `Editor`); mount a keyed [`CollabSession`]
/// to drive the slots per file.
pub fn use_collab_handles() -> CollabHandles {
    CollabHandles {
        doc: crate::doc_replica::use_replica_slot(),
        presence: crdt::use_presence_slot(),
        live: use_signal(|| false),
        pushed: use_signal(String::new),
        key: use_signal(|| format!("cursor-{}", Uuid::new_v4())),
        cursor_seq: use_signal(|| 0u64),
    }
}

/// Headless component driving one file's collab session **into** the
/// page-owned [`CollabHandles`]. Mount keyed by the doc id; reads the
/// page's [`DocumentSession`] from context.
#[component]
pub fn CollabSession(
    slug: String,
    vault_id: String,
    path: String,
    handles: CollabHandles,
) -> Element {
    let session = use_context::<DocumentSession>();
    let doc_id = vault_proto::collab_doc_id(&vault_id, &path);
    crate::doc_replica::use_replica_into(handles.doc, slug, vault_id, path);
    crdt::use_presence_channel_into(handles.presence, doc_id, CURSOR_TIMEOUT_MS);
    let replica = handles.doc;
    let live = handles.live;
    let pushed = handles.pushed;

    // Takeover + remote-apply. Runs on every doc revision (local commits
    // and remote imports both bump it) and when the replica turns ready.
    use_effect(move || {
        let _rev = replica.revision(); // subscribe
        let ready = replica.ready();
        let Some(doc) = replica.doc() else { return };
        let mut live = live;
        let mut pushed = pushed;

        // A replica discarded for foreign lineage: the person's buffer is
        // still here; take over again from the server's document (the
        // fold below keeps what they typed).
        if replica.take_discarded().is_some() && *live.peek() {
            live.set(false);
        }
        if !ready {
            return;
        }
        let buffer = session.state.peek().doc.to_string();

        if !*live.peek() {
            // ── Takeover ───────────────────────────────────────────
            // Reconcile editor buffer ⇄ replica exactly once, three-
            // way against the session's last committed text:
            //  - buffer == replica → nothing to do;
            //  - user hasn't typed since open → the replica (which
            //    includes peers' newer edits, and this device's offline
            //    ones) wins, the editor adopts it;
            //  - user typed during the handshake → fold the local edit
            //    (committed → buffer) into the replica as a change; the
            //    commit re-runs this effect in live mode, which settles
            //    the editor on the merged text.
            let remote = vault_doc::render(doc.loro(), Some(&buffer));
            if buffer != remote {
                let committed = session.committed_text();
                if buffer == committed {
                    apply_remote_to_editor(session.state, &remote);
                } else if !vault_doc::is_mid_edit(&buffer) {
                    vault_doc::merge_text(doc.loro(), &committed, &buffer);
                    vault_doc::commit_as(doc.loro(), "edited before the session attached");
                }
            }
            pushed.set(session.state.peek().doc.to_string());
            live.set(true);
            return;
        }

        // ── Steady state ───────────────────────────────────────────
        // After a local keystroke the document already renders as the
        // buffer (byte-stable render) → empty diff, no-op. After a remote
        // import the diff is the peers' edit. A half-typed frontmatter
        // waits: it would render back over the person's keystrokes.
        if vault_doc::is_mid_edit(&buffer) {
            return;
        }
        let remote = vault_doc::render(doc.loro(), Some(&buffer));
        apply_remote_to_editor(session.state, &remote);
        if *pushed.peek() != remote {
            pushed.set(remote);
        }
    });

    rsx! {}
}

/// Diff the editor buffer against `remote` and apply the delta as a
/// `"remote"`-tagged transaction. Host-driven `state.set` never re-enters
/// the `on_transaction` sink, and the tag is the second-line echo guard
/// (`event.is_remote()`).
fn apply_remote_to_editor(state: Signal<EditorState>, remote: &str) {
    let cur = state.peek().clone();
    let changes = editor_crdt::remote_text_to_changes(cur.doc.rope(), remote);
    if changes.is_empty() {
        return;
    }
    let mut state = state;
    state.set(cur.update(TransactionSpec::new().changes(changes).user_event("remote")));
}

/// Merge `new_text` (the whole buffer as it should now read) into the
/// live document as a change. Used for a host-side rewrite (the note
/// header's frontmatter splice) that must reach BOTH the shared doc and
/// the editor without a host-driven `state.set` (which would never reach
/// peers). The revision effect then echoes the merged text into the
/// editor. No-op before takeover.
pub fn push_full_text(c: &CollabHandles, new_text: &str) {
    if !*c.live.peek() {
        return;
    }
    let Some(doc) = c.doc.doc() else { return };
    let base = c.pushed.peek().clone();
    if vault_doc::merge_text(doc.loro(), &base, new_text).any() {
        vault_doc::commit_as(doc.loro(), "edited in the app");
    }
    let mut pushed = c.pushed;
    pushed.set(new_text.to_owned());
}

/// The vault page's `on_transaction` sink body: forward local edits into
/// the replica (echo-guarded) and publish the caret to the file's
/// presence channel (debounced).
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
        let after = event.doc_after.to_string();
        // A half-typed frontmatter is held back; the next edit that
        // parses carries it (the base stays the last pushed buffer).
        if !vault_doc::is_mid_edit(&after) {
            let base = c.pushed.peek().clone();
            if vault_doc::merge_text(doc.loro(), &base, &after).any() {
                vault_doc::commit_as(doc.loro(), "edited in the app");
            }
            let mut pushed = c.pushed;
            pushed.set(after);
        }
    }

    publish_cursor(c, session, event, who);
}

/// Scalar offset of the body inside a whole-file buffer — where the
/// document's `body` text starts. Zero when the buffer has no (valid)
/// frontmatter, matching how the codec reads it.
fn body_offset(buffer: &str) -> usize {
    match vault_doc::markdown::fences(buffer) {
        Some(f) if matches!(vault_doc::parse(buffer).front, vault_doc::Front::Props(_)) => {
            buffer[..f.body_start].chars().count()
        }
        _ => 0,
    }
}

/// Debounced presence publish: `{name, anchor_c, head_c}` where the
/// `_c` values are **encoded Loro stable cursors** minted against the
/// replica's `body` text, plus legacy `{anchor, head}` buffer scalars for
/// mixed-version peers. Fires for selection-only transactions too.
///
/// Stable cursors are the fix for "their caret walks backwards through
/// my word": a raw scalar published at time T stays fixed while MY local
/// inserts shift the text under it. A Loro cursor is anchored to the
/// character identity instead; every viewer resolves it against their
/// *own* current doc state (`get_cursor_pos`). A caret in the
/// frontmatter (not body text) is published as the body's start.
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
    let offset = body_offset(&event.doc_after.to_string());

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
            let text = vault_doc::doc::body_text(doc.loro());
            let len = text.len_unicode();
            let mut mint = |label: &str, pos: usize| {
                let at = pos.saturating_sub(offset).min(len);
                if let Some(cur) = text.get_cursor(at, crdt::loro::cursor::Side::Left) {
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

/// Apply scalar text ops to a text, clamping to the current length.
#[cfg(test)]
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
    // Stable cursors resolve to positions in the document's `body`;
    // our buffer has the frontmatter before it.
    let offset = body_offset(&state.doc.to_string());
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
                        return Some(q.current.pos + offset);
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

// (Registration — `VaultSync::open_collab` — is the replica driver's
// own per-session step now: `crate::doc_replica`.)

#[cfg(test)]
mod tests {
    use super::*;
    use editor::{Changes, Doc};

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
            let doc_before = Doc::new(text);
            let expected = changes.apply(&doc_before).to_string();

            let loro = crdt::loro::LoroDoc::new();
            let t = loro.get_text("t");
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
        let t = loro.get_text("t");
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
        let t = loro.get_text("t");
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
    fn body_offset_counts_the_frontmatter_in_scalars() {
        assert_eq!(body_offset("plain"), 0);
        let note = "---\ntitle: 🎸\n---\nbody";
        assert_eq!(body_offset(note), note.chars().count() - 4);
        // A frontmatter that does not parse is body, as the codec reads it.
        assert_eq!(body_offset("---\ntitle: [x\n---\nbody"), 0);
    }

    /// The editor bridge's round trip: each keystroke merges the buffer
    /// into the document against the last pushed buffer, and the
    /// document renders back to exactly the buffer — so the revision
    /// effect that follows a local edit is a no-op, keystroke after
    /// keystroke, frontmatter edits included.
    #[test]
    fn keystrokes_round_trip_through_the_document() {
        let doc = crdt::loro::LoroDoc::new();
        let mut pushed = "---\ntitle: \"Mix\"\ntags: [a, b]\n---\n# Mix\n".to_owned();
        vault_doc::adopt_text(&doc, &pushed);
        doc.commit();
        for next in [
            "---\ntitle: \"Mix\"\ntags: [a, b]\n---\n# Mix\nT",
            "---\ntitle: \"Mix\"\ntags: [a, b]\n---\n# Mix\nTake",
            "---\ntitle: \"Mix 2\"\ntags: [a, b]\n---\n# Mix\nTake",
            "---\ntitle: \"Mix 2\"\ntags: [a, b]\nstatus: open\n---\n# Mix\nTake",
        ] {
            assert!(!vault_doc::is_mid_edit(next));
            vault_doc::merge_text(&doc, &pushed, next);
            doc.commit();
            assert_eq!(vault_doc::render(&doc, Some(next)), next);
            pushed = next.to_owned();
        }
        // Half-typed: held back, not merged.
        assert!(vault_doc::is_mid_edit(
            "---\ntitle: \"Mix 2\n---\n# Mix\nTake"
        ));
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
