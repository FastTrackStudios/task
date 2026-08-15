//! `DocumentSession` — the open/save/conflict lifecycle for one
//! vault file, on architect-atom idioms.
//!
//! Replaces the hand-rolled `sha`/`dirty`/`conflict`/`saved_text`
//! signal cluster the vault page used to carry:
//!
//! - **Typed failures** — [`SaveError::Conflict`] carries the
//!   server's sha + text inline (mirroring
//!   `vault_proto::VaultSyncError::Conflict`), so the conflict
//!   banner renders from typed state, never from string matching.
//! - **Autosave** — every edit schedules a debounced save
//!   ([`AUTOSAVE_DEBOUNCE_MS`] after the last keystroke);
//!   further edits push it back. Suspendable via
//!   [`DocumentSession::set_autosave_paused`], and automatically
//!   held while a conflict is unresolved.
//! - **Explicit save / force save / reload** — Ctrl+S and the
//!   toolbar call [`DocumentSession::save`]; the conflict
//!   banner's *Overwrite* is [`DocumentSession::force_save`]
//!   (unconditional write) and *Reload* is
//!   [`DocumentSession::reload_from_server`] (adopt the server
//!   copy carried by the conflict).
//! - **Notifications** — non-conflict save failures and load
//!   errors report into `architect::Notifications` (provided at
//!   the app root by `architect::use_app_reactive`), so a
//!   failure outlives the page that caused it.
//!
//! Every signal is owned by the hook ([`use_document_session`]);
//! the session handle itself is `Copy` so callbacks capture it
//! freely.

use architect::try_use_notifications;
use dioxus::prelude::*;
use editor::EditorState;

/// The single vault id the server hosts per org. Referenced from
/// the wasm client calls (native arms are stubs, same as the rest
/// of the vault page's RPC helpers).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const VAULT_ID: &str = "default";

/// The scope that should own a pane's editor buffer.
///
/// A pane's [`EditorState`] does not stay inside the pane: `NoteView`
/// publishes it to the right sidebar as
/// [`FocusedDoc`](crate::pages::note_properties::FocusedDoc), stored in
/// a context signal provided by the *vault page* — an ancestor of both
/// the pane and the sidebar. The sidebar is a **sibling** subtree, not
/// a descendant of the pane.
///
/// Created with a plain `use_signal`, the buffer is owned by the pane's
/// own scope, so every sidebar read is a cross-scope read of a value
/// that dies when the pane unmounts (tab switch, file switch,
/// split-view swap). Dioxus flags exactly this ("A Copy Value created
/// in ScopeId(_, NoteView) … was used in ScopeId(_, NoteProperties)"),
/// and the multiplayer conformance suite fails the run on it — see
/// Finding 4 in `apps/task/tests/multiplayer/README.md`.
///
/// The page therefore hands panes *its* `ScopeId`, and the buffer is
/// created there instead. It outlives any single pane, so the sidebar's
/// reads are always valid. Because the pane's teardown then no longer
/// frees it, the pane must drop it explicitly — see
/// [`DocumentSession::dispose_buffer`].
#[derive(Clone, Copy)]
pub struct DocOwnerScope(pub ScopeId);

/// Debounce window between the last edit and the autosave write.
const AUTOSAVE_DEBOUNCE_MS: u64 = 2_000;

/// The server's copy of the file at the moment a conditional
/// write failed. Drives the Reload / Overwrite banner.
#[derive(Clone, PartialEq, Debug)]
pub struct Conflict {
    pub server_sha: String,
    pub server_text: String,
}

/// Typed outcome of a failed save attempt.
#[derive(Clone, PartialEq, Debug)]
pub enum SaveError {
    /// The conditional write lost: the file changed on the server
    /// since we opened/last saved it. Carries the server's
    /// current copy so resolution needs no second round-trip.
    Conflict {
        server_sha: String,
        server_text: String,
    },
    /// Transport / IO / anything else, stringly (the vox error
    /// chain bottoms out in debug formatting today).
    Other(String),
}

/// Where the session is in its save lifecycle. `Failed` keeps the
/// human-readable reason for the toolbar; the structured conflict
/// payload lives in [`DocumentSession::conflict`].
#[derive(Clone, PartialEq, Debug, Default)]
pub enum SaveStatus {
    #[default]
    Idle,
    Saving,
    Saved,
    Failed(String),
}

/// The open file's wire identity: path + last-known server sha.
/// `sha == None` means the buffer has never been committed
/// (a create-only write).
#[derive(Clone, PartialEq)]
struct OpenFile {
    path: String,
    sha: Option<String>,
}

/// `Copy` handle over the session's signals. Create one per page
/// via [`use_document_session`].
#[derive(Clone, Copy)]
pub struct DocumentSession {
    /// Active org slug (the vault lives in the home org).
    home: Memo<String>,
    /// The editor buffer. Pass straight to `editor::Editor`.
    pub state: Signal<EditorState>,
    open: Signal<Option<OpenFile>>,
    saved_text: Signal<String>,
    status: Signal<SaveStatus>,
    conflict: Signal<Option<Conflict>>,
    autosave_paused: Signal<bool>,
    /// Open-generation counter — bumped on every `open`/`seed`,
    /// checked after every await so a stale completion can't
    /// clobber a newer document.
    epoch: Signal<u64>,
    /// Edit-generation counter for the autosave debounce: each
    /// edit schedules a task tagged with the new value; only the
    /// task whose tag is still current fires.
    edit_seq: Signal<u64>,
    /// One save in flight at a time.
    saving: Signal<bool>,
    /// Bumped on every successful save. Resources that should
    /// refresh after a commit (e.g. the backlinks panel) read it
    /// reactively.
    save_count: Signal<u64>,
    notify: Option<architect::Notifications>,
    /// True when [`state`](Self::state) was created in an ancestor
    /// scope via [`DocOwnerScope`], and so is NOT freed when this pane
    /// unmounts. The pane owns the responsibility instead — see
    /// [`DocumentSession::dispose_buffer`].
    buffer_is_foreign: bool,
}

/// Create the session and install the debounced-autosave effect.
pub fn use_document_session(home: Memo<String>) -> DocumentSession {
    // The buffer is the one piece of session state that leaves this
    // scope (see [`DocOwnerScope`]), so it is created in the page's
    // scope when the page offers one. Everything below stays local:
    // nothing else here is read outside the pane.
    let owner = try_use_context::<DocOwnerScope>();
    let state = use_hook(|| match owner {
        Some(DocOwnerScope(scope)) => Signal::new_in_scope(EditorState::new(String::new()), scope),
        // Standalone mount (no vault page above): local ownership is
        // correct and `dispose_buffer` becomes a no-op.
        None => Signal::new(EditorState::new(String::new())),
    });
    let open = use_signal(|| None::<OpenFile>);
    let saved_text = use_signal(String::new);
    let status = use_signal(SaveStatus::default);
    let conflict = use_signal(|| None::<Conflict>);
    let autosave_paused = use_signal(|| false);
    let epoch = use_signal(|| 0u64);
    let edit_seq = use_signal(|| 0u64);
    let saving = use_signal(|| false);
    let save_count = use_signal(|| 0u64);
    let notify = try_use_notifications();

    let session = DocumentSession {
        home,
        state,
        open,
        saved_text,
        status,
        conflict,
        autosave_paused,
        epoch,
        edit_seq,
        saving,
        save_count,
        notify,
        buffer_is_foreign: owner.is_some(),
    };

    // Debounced autosave. Subscribes to the editor state; every
    // write while dirty schedules a save AUTOSAVE_DEBOUNCE_MS out
    // tagged with a fresh edit_seq — a newer edit supersedes it.
    // The spawned task is scoped to this component, so navigating
    // away cancels any pending autosave.
    use_effect(move || {
        let cur = session.state.read().doc.to_string();
        if session.open.peek().is_none() || cur == *session.saved_text.peek() {
            return;
        }
        let seq = session.edit_seq.peek().wrapping_add(1);
        {
            let mut edit_seq = session.edit_seq;
            edit_seq.set(seq);
        }
        spawn(async move {
            architect::sleep(architect::Duration::from_millis(AUTOSAVE_DEBOUNCE_MS)).await;
            if *session.edit_seq.peek() != seq
                || *session.autosave_paused.peek()
                || session.conflict.peek().is_some()
            {
                return;
            }
            session.save_now(false).await;
        });
    });

    session
}

impl DocumentSession {
    /// Free the editor buffer when it was created in the page's scope
    /// rather than the pane's (see [`DocOwnerScope`]).
    ///
    /// The pane MUST call this as it unmounts, *after* withdrawing
    /// itself from the focused-doc context — otherwise the sidebar
    /// could read a buffer that has already been freed. Without it the
    /// buffer would live until the whole vault page unmounts, leaking
    /// one document per tab/file switch.
    ///
    /// A no-op when the buffer is locally owned, which is the case for
    /// a `NoteView` mounted outside the vault page.
    pub fn dispose_buffer(&self) {
        if self.buffer_is_foreign {
            self.state.manually_drop();
        }
    }

    // ── Reactive reads ────────────────────────────────────────

    /// Path of the open file, or `None`. Reactive.
    pub fn current_path(&self) -> Option<String> {
        self.open.read().as_ref().map(|o| o.path.clone())
    }

    /// Buffer differs from the last committed text. Reactive.
    pub fn dirty(&self) -> bool {
        self.open.read().is_some() && self.state.read().doc.to_string() != *self.saved_text.read()
    }

    /// Unresolved conflict, when a conditional write lost.
    /// Reactive.
    pub fn conflict(&self) -> Option<Conflict> {
        self.conflict.read().clone()
    }

    /// Save lifecycle phase. Reactive.
    pub fn status(&self) -> SaveStatus {
        self.status.read().clone()
    }

    /// Successful-save counter — read this reactively from
    /// resources that should refresh after each commit.
    pub fn save_count(&self) -> u64 {
        *self.save_count.read()
    }

    /// The last committed text (what was fetched on open, or what the
    /// last successful save wrote). Non-reactive — the collab takeover
    /// uses it as the three-way base separating "user typed since the
    /// open" from "the server doc is ahead of our fetch".
    pub(crate) fn committed_text(&self) -> String {
        self.saved_text.peek().clone()
    }

    // ── Controls ──────────────────────────────────────────────

    /// Suspend / resume the autosave debounce. Explicit saves
    /// still work while paused.
    pub fn set_autosave_paused(&self, paused: bool) {
        let mut p = self.autosave_paused;
        p.set(paused);
    }

    /// Open `path`: fetch its bytes, seed a fresh editor state,
    /// and remember `sha` so the next save is a conditional
    /// write. Load failures go to notifications.
    pub fn open(&self, path: String, sha: String) {
        let s = *self;
        let ep = s.bump_epoch();
        spawn(async move {
            match fetch_file(s.home.peek().clone(), path.clone()).await {
                Ok(text) => {
                    if *s.epoch.peek() != ep {
                        return; // a newer open superseded this one
                    }
                    s.seed_inner(path, Some(sha), text);
                }
                Err(e) => {
                    if *s.epoch.peek() != ep {
                        return;
                    }
                    if let Some(n) = s.notify {
                        n.error(format!("Couldn't open {path}: {e}"));
                    }
                }
            }
        });
    }

    /// Seed the session without a fetch — for buffers whose
    /// content is already known (a just-created empty note).
    /// `sha == None` marks a never-committed buffer (the first
    /// save is create-only).
    pub fn seed(&self, path: String, sha: Option<String>, text: String) {
        self.bump_epoch();
        self.seed_inner(path, sha, text);
    }

    /// Explicit save (Ctrl+S / toolbar). Conditional on the
    /// last-known sha; conflicts surface in
    /// [`Self::conflict`].
    pub fn save(&self) {
        let s = *self;
        spawn(async move { s.save_now(false).await });
    }

    /// Unconditional write — the conflict banner's *Overwrite*.
    pub fn force_save(&self) {
        let s = *self;
        spawn(async move { s.save_now(true).await });
    }

    /// Resolve a conflict by adopting the server's copy (the
    /// banner's *Reload*). Local edits are discarded.
    pub fn reload_from_server(&self) {
        let Some(c) = self.conflict.peek().clone() else {
            return;
        };
        let path = match self.open.peek().as_ref() {
            Some(o) => o.path.clone(),
            None => return,
        };
        self.bump_epoch();
        self.seed_inner(path, Some(c.server_sha), c.server_text);
        if let Some(n) = self.notify {
            n.info("Reloaded from server");
        }
    }

    // ── Internals ─────────────────────────────────────────────

    fn bump_epoch(&self) -> u64 {
        let mut epoch = self.epoch;
        let v = epoch.peek().wrapping_add(1);
        epoch.set(v);
        v
    }

    fn seed_inner(&self, path: String, sha: Option<String>, text: String) {
        let mut state = self.state;
        let mut saved_text = self.saved_text;
        let mut open = self.open;
        let mut conflict = self.conflict;
        let mut status = self.status;
        saved_text.set(text.clone());
        state.set(EditorState::new(text));
        open.set(Some(OpenFile { path, sha }));
        conflict.set(None);
        status.set(SaveStatus::Idle);
    }

    /// One save attempt. Clean buffers are a no-op unless
    /// `force` — in a conflict the buffer can equal our base
    /// while the server differs, so Overwrite must always write.
    async fn save_now(self, force: bool) {
        if *self.saving.peek() {
            return; // one in flight; the autosave debounce retries
        }
        let Some(of) = self.open.peek().clone() else {
            return;
        };
        let text = self.state.peek().doc.to_string();
        if !force && text == *self.saved_text.peek() {
            return; // clean buffer — nothing to commit
        }
        let ep = *self.epoch.peek();
        {
            let mut saving = self.saving;
            saving.set(true);
            let mut status = self.status;
            status.set(SaveStatus::Saving);
        }
        let result = save_file(
            self.home.peek().clone(),
            of.path.clone(),
            text.clone(),
            of.sha.clone(),
            force,
        )
        .await;
        {
            let mut saving = self.saving;
            saving.set(false);
        }
        if *self.epoch.peek() != ep {
            return; // a different document was opened mid-flight
        }
        let mut status = self.status;
        match result {
            Ok(new_sha) => {
                let mut open = self.open;
                let mut saved_text = self.saved_text;
                let mut conflict = self.conflict;
                let mut save_count = self.save_count;
                open.set(Some(OpenFile {
                    path: of.path,
                    sha: Some(new_sha),
                }));
                saved_text.set(text);
                conflict.set(None);
                status.set(SaveStatus::Saved);
                let n = save_count.peek().wrapping_add(1);
                save_count.set(n);
            }
            Err(SaveError::Conflict {
                server_sha,
                server_text,
            }) => {
                let mut conflict = self.conflict;
                conflict.set(Some(Conflict {
                    server_sha,
                    server_text,
                }));
                status.set(SaveStatus::Failed(
                    "Conflict — file changed on server".to_owned(),
                ));
            }
            Err(SaveError::Other(e)) => {
                status.set(SaveStatus::Failed(format!("Save failed: {e}")));
                if let Some(n) = self.notify {
                    n.error(format!("Save failed for {}: {e}", of.path));
                }
            }
        }
    }
}

// ── RPC helpers (wasm-only bodies, same shape as the rest of the
//    vault page's client calls) ───────────────────────────────────

/// Read one file's bytes as UTF-8 text. Shared with the
/// vault-lookup content cache.
pub(crate) async fn fetch_file(slug: String, path: String) -> Result<String, String> {
    let client = crate::vox_clients::vault_client(&slug).await?;
    #[cfg(target_arch = "wasm32")]
    {
        let bytes = client
            .get_file(VAULT_ID.to_owned(), path)
            .await
            .map_err(|e| format!("get_file: {e:?}"))?;
        Ok(String::from_utf8_lossy(&bytes.0).into_owned())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (client, path);
        Err("native client not wired yet".to_owned())
    }
}

/// Conditional-write the file back. `prev_sha` is the last-known
/// server hash (`None` → create-only); `force` writes
/// unconditionally. Returns the freshly committed sha.
async fn save_file(
    slug: String,
    path: String,
    text: String,
    prev_sha: Option<String>,
    force: bool,
) -> Result<String, SaveError> {
    let client = crate::vox_clients::vault_client(&slug)
        .await
        .map_err(SaveError::Other)?;
    #[cfg(target_arch = "wasm32")]
    {
        use vault_proto::{IfMatch, VaultSyncError};
        use vox::VoxError;
        let if_match = if force {
            IfMatch::Force
        } else {
            match prev_sha {
                Some(s) => IfMatch::Sha(s),
                None => IfMatch::CreateOnly,
            }
        };
        match client
            .put_file(VAULT_ID.to_owned(), path, text.into_bytes(), if_match)
            .await
        {
            Ok(ack) => Ok(ack.sha256),
            Err(VoxError::User(user)) => match *user {
                VaultSyncError::Conflict {
                    server_sha,
                    server_bytes,
                } => Err(SaveError::Conflict {
                    server_sha,
                    server_text: String::from_utf8_lossy(&server_bytes).into_owned(),
                }),
                other => Err(SaveError::Other(format!("{other:?}"))),
            },
            Err(e) => Err(SaveError::Other(format!("{e:?}"))),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (client, path, text, prev_sha, force);
        Err(SaveError::Other("native client not wired yet".to_owned()))
    }
}
