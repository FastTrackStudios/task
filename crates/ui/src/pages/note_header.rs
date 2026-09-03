//! Note header — the editable title rendered ABOVE the editor body
//! inside `/vault`.
//!
//! It's just the note's **name**: an H1 showing the filename basename;
//! committing a change RENAMES the file (`put_file` create-only at the
//! new path → navigate → `delete_file` the old path). Properties are the
//! editor's own built-in frontmatter widget (rendered from the leading
//! `---…---` block) — the header stays out of that and simply blends
//! into the top of the page, Obsidian-style, with no separators.

use dioxus::prelude::*;

use crate::document_session::DocumentSession;
use crate::pages::vault::basename_of;

/// Header rendered above the editor body: the note's editable title.
///
/// The [`DocumentSession`] comes from context (provided by the vault
/// page, same as [`crate::collab::CollabSession`]) since it isn't
/// `PartialEq` and so can't be a memoised component prop.
#[component]
pub fn NoteHeader(
    /// The home-org slug the note lives under (for the rename RPCs).
    home: Memo<String>,
    /// Shared with the vault page: when `false`, the editor wrapper gets
    /// `props-collapsed` and hides the frontmatter properties widget.
    /// The chevron below the title toggles it.
    props_open: Signal<bool>,
    /// Bumped by the note view when keyboard focus should move up into
    /// the title (caret at the document top + `k`/ArrowUp). Entering
    /// edit mode selects the whole title so typing replaces it.
    focus_req: Signal<u32>,
    /// Refresh the folder index after a rename commits (the tree row
    /// path changed).
    on_renamed: EventHandler<()>,
) -> Element {
    let session = use_context::<DocumentSession>();
    let notify = architect::try_use_notifications();
    let nav = use_navigator();

    let Some(path) = session.current_path() else {
        return rsx! {};
    };
    let title = basename_of(&path).to_owned();

    // A freshly-created "Untitled" note (the `<space> n` flow) asks to
    // be named immediately: the new-note intent parks the path in
    // `PendingTitleEdit`, and the matching header consumes it by
    // requesting title focus.
    {
        let mut pending = use_context::<crate::chrome::PendingTitleEdit>().0;
        let mut focus_req = focus_req;
        let my_path = path.clone();
        use_effect(move || {
            let hit = pending.read().as_deref() == Some(my_path.as_str());
            if hit {
                pending.set(None);
                focus_req += 1;
            }
        });
    }

    // ── Rename: title commit → put_file(new, CreateOnly) → nav → delete.
    let cur_title = title.clone();
    let do_rename = move |new_title: String| {
        let new_title = new_title.trim().to_owned();
        if new_title.is_empty() || new_title == cur_title {
            return;
        }
        let Some(new_path) = rename_path(&path, &new_title) else {
            if let Some(n) = notify {
                n.error("Invalid note name");
            }
            return;
        };
        if new_path == path {
            return;
        }
        let old_path = path.clone();
        let bytes = session.state.peek().doc.to_string().into_bytes();
        let slug = home.peek().clone();
        let vault_id = session.vault_id();
        spawn(async move {
            // Save any pending edits to the OLD path first so nothing is
            // lost if the create fails and we stay put. Best-effort.
            session.save();
            match put_file_create(slug.clone(), vault_id.clone(), new_path.clone(), bytes).await {
                Ok(_) => {
                    // A vault note stays on the vault route (org from
                    // the switcher); a wiki page stays on its wiki's.
                    nav.push(crate::routes::note_route(
                        if crate::document_session::wiki_of_vault_id(&vault_id).is_some() {
                            &slug
                        } else {
                            ""
                        },
                        &vault_id,
                        new_path.clone(),
                    ));
                    // The new file is committed and we've navigated; drop
                    // the old one. A delete failure only leaves a stale
                    // copy behind — surface it but don't block.
                    if let Err(e) = delete_file(slug, vault_id, old_path).await {
                        if let Some(n) = notify {
                            n.error(format!("Renamed, but couldn't remove the old note: {e}"));
                        }
                    }
                    on_renamed.call(());
                }
                Err(e) => {
                    if let Some(n) = notify {
                        n.error(format!("Rename failed: {e}"));
                    }
                }
            }
        });
    };

    // Properties now live in the right sidebar (Properties tab), so the
    // header is just the note's editable title. `props_open` is retained
    // for the caller's `props-collapsed` class (always collapsed).
    let _ = props_open;
    rsx! {
        div { class: "flex flex-col gap-1 px-6 pt-5 pb-0",
            TitleField { title, focus_req, on_commit: do_rename }
        }
    }
}

/// Refocus the editor body (the contenteditable root) — the title's
/// keyboard exit (ArrowDown / Escape) drops back into the document.
fn focus_editor_body() {
    let _ = dioxus::document::eval(
        "const el = document.querySelector('.editor-root'); if (el) el.focus();",
    );
}

/// Big, click-to-edit title styled like an H1. Click swaps the heading
/// for an input; Enter or blur commits, Escape cancels, ArrowDown
/// commits and returns focus to the document. A `focus_req` bump
/// enters edit mode with the whole title selected (type to replace).
#[component]
fn TitleField(title: String, focus_req: Signal<u32>, on_commit: EventHandler<String>) -> Element {
    let mut editing = use_signal(|| false);
    let mut draft = use_signal(String::new);
    // Whether the next mount of the input should select-all (keyboard
    // entry) rather than leave the caret where the click put it.
    let mut select_all = use_signal(|| false);

    {
        let title = title.clone();
        use_effect(move || {
            if focus_req() > 0 && !*editing.peek() {
                draft.set(title.clone());
                select_all.set(true);
                editing.set(true);
            }
        });
    }

    if editing() {
        rsx! {
            input {
                id: "note-title-input",
                class: "w-full bg-transparent text-3xl font-bold tracking-tight text-foreground outline-none",
                value: "{draft}",
                autofocus: true,
                onmounted: move |el| async move {
                    let _ = el.set_focus(true).await;
                    if *select_all.peek() {
                        select_all.set(false);
                        let _ = dioxus::document::eval(
                            "const i = document.getElementById('note-title-input'); if (i) i.select();",
                        );
                    }
                },
                oninput: move |e| draft.set(e.value()),
                onkeydown: move |e: KeyboardEvent| {
                    e.stop_propagation();
                    if e.key() == Key::Enter {
                        editing.set(false);
                        on_commit.call(draft.peek().clone());
                        focus_editor_body();
                    } else if e.key() == Key::Escape {
                        editing.set(false);
                        focus_editor_body();
                    } else if e.key() == Key::ArrowDown {
                        // The document continues below the title —
                        // commit and drop focus back into it.
                        editing.set(false);
                        on_commit.call(draft.peek().clone());
                        focus_editor_body();
                    }
                },
                onfocusout: move |_| {
                    if *editing.peek() {
                        editing.set(false);
                        on_commit.call(draft.peek().clone());
                    }
                },
            }
        }
    } else {
        let display = title.clone();
        rsx! {
            button {
                class: "-mx-1 w-full truncate rounded px-1 text-left text-3xl font-bold tracking-tight text-foreground hover:bg-accent/40",
                title: "Click to rename",
                onclick: move |_| {
                    draft.set(display.clone());
                    editing.set(true);
                },
                "{title}"
            }
        }
    }
}

// ── Rename path helpers ────────────────────────────────────────────

/// The new vault path for a renamed note: same folder, `title` as the
/// filename with a `.md` extension. Returns `None` if nothing usable
/// survives sanitisation.
fn rename_path(old_path: &str, title: &str) -> Option<String> {
    let name = sanitize_filename(title);
    if name.is_empty() {
        return None;
    }
    let file = format!("{name}.md");
    Some(match old_path.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/{file}"),
        None => file,
    })
}

/// Keep the title filesystem-safe: drop path separators and characters
/// that break paths on common filesystems, collapse whitespace.
fn sanitize_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    // Collapse runs of whitespace, trim, and strip a trailing `.md` the
    // user may have typed (we re-append it).
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .strip_suffix(".md")
        .unwrap_or(&collapsed)
        .trim()
        .to_owned()
}

// ── RPC helpers (wasm-only bodies, mirroring the vault page) ───────

/// Create-only write of `bytes` at `path` (the rename target). Fails if
/// the target already exists — the caller surfaces that as a toast and
/// keeps the old file.
async fn put_file_create(
    slug: String,
    vault_id: String,
    path: String,
    bytes: Vec<u8>,
) -> Result<String, String> {
    let client = crate::vox_clients::vault_client(&slug).await?;
    #[cfg(target_arch = "wasm32")]
    {
        use vault_proto::IfMatch;
        let ack = client
            .put_file(vault_id, path, bytes, IfMatch::CreateOnly)
            .await
            .map_err(|e| format!("put_file: {e:?}"))?;
        Ok(ack.sha256)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (client, vault_id, path, bytes);
        Err("native client not wired yet".to_owned())
    }
}

/// Delete `path` unconditionally (the old name after a rename).
async fn delete_file(slug: String, vault_id: String, path: String) -> Result<(), String> {
    let client = crate::vox_clients::vault_client(&slug).await?;
    #[cfg(target_arch = "wasm32")]
    {
        use vault_proto::IfMatch;
        client
            .delete_file(vault_id, path, IfMatch::Force)
            .await
            .map_err(|e| format!("delete_file: {e:?}"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (client, vault_id, path);
        Err("native client not wired yet".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_keeps_folder_and_sanitizes() {
        assert_eq!(
            rename_path("Notes/Old Name.md", "New Title").as_deref(),
            Some("Notes/New Title.md")
        );
        assert_eq!(
            rename_path("Untitled 1a2b.md", "a/b:c").as_deref(),
            Some("a b c.md")
        );
        assert_eq!(
            rename_path("x.md", "My Note.md").as_deref(),
            Some("My Note.md")
        );
        assert_eq!(rename_path("x.md", "   "), None);
    }
}
