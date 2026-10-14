//! One document editor for tasks and notes.
//!
//! [`DocBody`] is the real markdown editor (`editor::Editor` plus its `/`
//! palette) with nothing of the vault attached: no file, no collab, no
//! frontmatter. A host hands it a body as a string and hears every change
//! back as a string, and decides for itself how that persists — the issue
//! page commits it through the task mutation path, debounced
//! ([`use_debounced_commit`]). The vault's `NoteView` keeps its own
//! session-bound `Editor` (collab, presence, widgets), but it wears the same
//! [`DOC_CLASS`], the same [`DOC_CSS`], the same keymap ([`doc_keymap`]) and
//! the same slash catalog ([`install_palette`]), so a note and a task's
//! description read and type the same.
//!
//! What sits here, all shared by both mounts:
//!
//! - **Typography** ([`DOC_CSS`]) — Linear's measurements on the live
//!   markdown editor: 15/24 body at weight 450, 22/19/16 headings, a 16px
//!   gap where a blank line separates blocks, quiet code and quotes. The
//!   markdown marks stay (that is our edge over Linear) and dim instead of
//!   competing with the text.
//! - **The slash menu** ([`doc_commands`]) — Linear's groups first (Text,
//!   Insert) with their shortcuts on the right, then the markdown extras
//!   the wiki needs (study blocks, callout kinds, links, math).
//! - **The shortcuts** ([`doc_keymap`]) — the same chords the menu shows.
//!
//! Nothing here is wasm-only: the editor runs in the iOS webview, the
//! desktop webview and the browser alike.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use dioxus::prelude::*;
use editor::editor_view::palette::{
    CommandEntry, CommandKind, CommandPalette, ListKind, PaletteState, markdown_commands,
    register_catalog, run_command, set_trigger,
};
use editor::{Changes, Editor, EditorState, Keymap, Selection, TransactionSpec};

/// The class every document surface carries — the scope of [`DOC_CSS`].
pub const DOC_CLASS: &str = "doc-linear";

/// Use `/` for the palette and register the document catalog, once per
/// process. Both the note view and [`DocBody`] call it; the second call is
/// free.
// t[impl doc.editor.slash]
pub fn install_palette() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // Prose, so `/` (at a line start or after a space — never in
        // `and/or` or a URL). The editor's own default is `\` for Keyflow.
        set_trigger('/');
        register_catalog(doc_commands());
    });
}

// ── The slash catalog ───────────────────────────────────────────────

/// A Linear-group entry: label, group, shortcut shown on the right, the
/// command, and the badge.
const fn entry(
    label: &'static str,
    group: &'static str,
    shortcut: &'static str,
    kind: CommandKind,
    icon: &'static str,
) -> CommandEntry {
    CommandEntry {
        label,
        group,
        desc: shortcut,
        kind,
        icon,
    }
}

const CODE_BLOCK: CommandKind = CommandKind::InsertBlockSnippet("```\n\n```\n", 5);
const DIAGRAM: CommandKind = CommandKind::InsertBlockSnippet("```mermaid\n\n```\n", 5);
const COLLAPSIBLE: CommandKind =
    CommandKind::InsertBlockSnippet("> [!note]- ${Section title}\n> \n", 0);
const CALLOUT: CommandKind = CommandKind::InsertBlockSnippet("> [!note] ", 0);
// A blank line first, so the rule never sits under a paragraph line where
// `---` would read as a setext heading.
const DIVIDER: CommandKind = CommandKind::InsertBlockSnippet("\n---\n", 0);
const TABLE: CommandKind = CommandKind::InsertBlockSnippet(
    "| Column | Column |\n| ------ | ------ |\n| ${Cell} |        |\n",
    0,
);
const IMAGE: CommandKind = CommandKind::InsertSnippet("![](${https://})", 0);
// No upload path for a task body yet: a link placeholder to fill in
// (docs/spec/unmet.md, `doc.editor.attach`).
const ATTACH: CommandKind = CommandKind::InsertSnippet("[${File name}](url)", 0);
const BLOCKQUOTE: CommandKind = CommandKind::InsertSnippet("> ", 0);

/// The commands the Linear groups replace in the editor's own catalog —
/// left out of the extras so nothing is listed twice.
const SUPERSEDED: &[&str] = &[
    "Heading 1",
    "Heading 2",
    "Heading 3",
    "Bulleted list",
    "Numbered list",
    "Task",
    "Quote",
    "Horizontal rule",
    "Table",
    "Code block",
    "Mermaid diagram",
    "Note",
];

/// The document palette: Linear's Text and Insert groups, in Linear's
/// order with Linear's shortcuts, then what markdown and the wiki add.
#[must_use]
pub fn doc_commands() -> Vec<CommandEntry> {
    let mut out = vec![
        entry("Heading 1", "Text", "⌘⌥1", CommandKind::SetHeading(1), "H1"),
        entry("Heading 2", "Text", "⌘⌥2", CommandKind::SetHeading(2), "H2"),
        entry("Heading 3", "Text", "⌘⌥3", CommandKind::SetHeading(3), "H3"),
        entry(
            "Bulleted list",
            "Text",
            "⌘⇧8",
            CommandKind::SetList(ListKind::Unordered),
            "•",
        ),
        entry(
            "Numbered list",
            "Text",
            "⌘⇧9",
            CommandKind::SetList(ListKind::Ordered),
            "1.",
        ),
        entry(
            "Checklist",
            "Text",
            "⌘⇧7",
            CommandKind::SetList(ListKind::Task),
            "[ ]",
        ),
        entry("Image", "Insert", "", IMAGE, "img"),
        entry("Attach files", "Insert", "⌘⇧U", ATTACH, "att"),
        entry("Code block", "Insert", "⌘⇧\\", CODE_BLOCK, "{}"),
        entry("Diagram", "Insert", "mermaid", DIAGRAM, "~>"),
        entry("Collapsible section", "Insert", "⌘⇧6", COLLAPSIBLE, ">-"),
        entry("Blockquote", "Insert", "⌥⇧.", BLOCKQUOTE, ">"),
        entry("Callout", "Insert", "", CALLOUT, "!"),
        entry("Divider", "Insert", "---", DIVIDER, "—"),
        entry("Table", "Insert", "", TABLE, "tbl"),
    ];
    let extras: Vec<CommandEntry> = markdown_commands()
        .into_iter()
        .filter(|c| !SUPERSEDED.contains(&c.label))
        // The extras keep their descriptions only where they say
        // something the label does not (the study blocks); syntax
        // reminders like `- item` would crowd the shortcut column.
        .map(|mut c| {
            if c.group != "Study" {
                c.desc = "";
            }
            c
        })
        .collect();
    // The menu prints a header whenever the group changes, so each group
    // must be contiguous: gather the extras by group, in the order the
    // groups first appear (the editor's own catalog interleaves Code and
    // Math).
    let mut groups: Vec<&str> = Vec::new();
    for c in &extras {
        if !groups.contains(&c.group) {
            groups.push(c.group);
        }
    }
    for g in groups {
        out.extend(extras.iter().filter(|c| c.group == g).cloned());
    }
    out
}

// ── The shortcuts ───────────────────────────────────────────────────

fn caret_of(s: &EditorState) -> usize {
    s.selection.primary().head
}

fn run_here(s: &EditorState, kind: CommandKind) -> Option<TransactionSpec> {
    let at = caret_of(s);
    run_command(s, at..at, kind)
}

fn h1(s: &EditorState) -> Option<TransactionSpec> {
    editor::commands::set_heading(s, 1)
}
fn h2(s: &EditorState) -> Option<TransactionSpec> {
    editor::commands::set_heading(s, 2)
}
fn h3(s: &EditorState) -> Option<TransactionSpec> {
    editor::commands::set_heading(s, 3)
}
fn bullets(s: &EditorState) -> Option<TransactionSpec> {
    run_here(s, CommandKind::SetList(ListKind::Unordered))
}
fn numbers(s: &EditorState) -> Option<TransactionSpec> {
    run_here(s, CommandKind::SetList(ListKind::Ordered))
}
fn checklist(s: &EditorState) -> Option<TransactionSpec> {
    run_here(s, CommandKind::SetList(ListKind::Task))
}
fn code_block(s: &EditorState) -> Option<TransactionSpec> {
    run_here(s, CODE_BLOCK)
}
fn collapsible(s: &EditorState) -> Option<TransactionSpec> {
    run_here(s, COLLAPSIBLE)
}
fn attach(s: &EditorState) -> Option<TransactionSpec> {
    run_here(s, ATTACH)
}
/// `> ` at the start of the caret's line (a second press is harmless:
/// the editor reads `> > ` as a nested quote, which is what was asked).
fn blockquote(s: &EditorState) -> Option<TransactionSpec> {
    let doc = s.doc.to_string();
    let at = caret_of(s).min(doc.len());
    let line_start = doc[..at].rfind('\n').map_or(0, |n| n + 1);
    Some(
        TransactionSpec::new()
            .changes(Changes::insert(line_start, "> "))
            .selection(Selection::caret(at + 2))
            .annotate("origin", "shortcut"),
    )
}

/// The document keymap: the standard markdown bindings plus the chords
/// the slash menu shows. Each chord is bound under every key a browser
/// may report for it — on a Mac, ⌥ changes the character (`⌘⌥1` arrives
/// as `¡`) and ⇧ shifts it (`⌘⇧8` arrives as `*`).
// t[impl doc.editor.shortcuts]
#[must_use]
pub fn doc_keymap() -> Keymap {
    let mut k = Keymap::new();
    let binds: &[(&[&str], editor::Command)] = &[
        (&["Mod-Alt-1", "Mod-Alt-¡"], h1),
        (&["Mod-Alt-2", "Mod-Alt-™"], h2),
        (&["Mod-Alt-3", "Mod-Alt-£"], h3),
        (&["Mod-Shift-8", "Mod-Shift-*"], bullets),
        (&["Mod-Shift-9", "Mod-Shift-("], numbers),
        (&["Mod-Shift-7", "Mod-Shift-&"], checklist),
        (&["Mod-Shift-\\", "Mod-Shift-|"], code_block),
        (&["Mod-Shift-6", "Mod-Shift-^"], collapsible),
        (&["Mod-Shift-u"], attach),
        (&["Alt-Shift-.", "Alt-Shift->", "Alt-Shift-˘"], blockquote),
    ];
    for (keys, run) in binds {
        for key in *keys {
            k = k.with(*key, *run);
        }
    }
    for b in editor::standard_markdown_keymap().bindings() {
        k = k.with(b.key.clone(), b.run);
    }
    k
}

// ── Keeping a host's text and the buffer in step ────────────────────

/// What to do with a body the host just handed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Incoming {
    /// The buffer already says this — the echo of our own edit.
    InSync,
    /// Someone else changed it and nothing local is outstanding: take it.
    Apply,
    /// Someone else changed it while local edits are still unsaved: keep
    /// the local text. It is written over theirs on the next commit (last
    /// writer wins, the task service's own rule for a whole record).
    KeepLocal,
}

/// Decide on `incoming` given the `buffer` and the text both sides last
/// `agreed` on.
// t[impl doc.editor.body-only]
#[must_use]
pub fn reconcile(buffer: &str, agreed: &str, incoming: &str) -> Incoming {
    if incoming == buffer {
        Incoming::InSync
    } else if buffer == agreed {
        Incoming::Apply
    } else {
        Incoming::KeepLocal
    }
}

/// The one replace that turns `old` into `new`: the differing middle,
/// between the longest common prefix and suffix, on char boundaries. So
/// an incoming change lands as an edit the editor maps the caret through,
/// rather than a whole-document swap that throws it to the top.
#[must_use]
pub fn minimal_replace(old: &str, new: &str) -> Option<(Range<usize>, String)> {
    if old == new {
        return None;
    }
    let prefix = old
        .char_indices()
        .zip(new.chars())
        .find(|((_, a), b)| a != b)
        .map_or_else(|| old.len().min(new.len()), |((i, _), _)| i);
    // Never let the suffix overlap the prefix on either side.
    let max_suffix = (old.len() - prefix).min(new.len() - prefix);
    let mut suffix = 0;
    for (a, b) in old[prefix..].chars().rev().zip(new[prefix..].chars().rev()) {
        if a != b || suffix + a.len_utf8() > max_suffix {
            break;
        }
        suffix += a.len_utf8();
    }
    Some((
        prefix..old.len() - suffix,
        new[prefix..new.len() - suffix].to_owned(),
    ))
}

// ── Debounced commit ────────────────────────────────────────────────

/// An edit waiting to be committed: which document, what text, and a
/// generation so only the newest timer commits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingEdit {
    generation: u64,
    held: Option<(String, String)>,
}

impl PendingEdit {
    /// Hold `text` for `key`. Returns the timer's generation, and the edit
    /// held for a DIFFERENT document, which must be committed now (one
    /// document's text never waits under another's key).
    pub fn push(&mut self, key: &str, text: String) -> (u64, Option<(String, String)>) {
        let other = match &self.held {
            Some((k, _)) if k != key => self.held.take(),
            _ => None,
        };
        self.generation += 1;
        self.held = Some((key.to_owned(), text));
        (self.generation, other)
    }

    /// The held edit, if `generation` is still the newest timer.
    pub fn take_if(&mut self, generation: u64) -> Option<(String, String)> {
        if generation == self.generation {
            self.held.take()
        } else {
            None
        }
    }

    /// Whatever is held, now (blur, close, unmount).
    pub fn take(&mut self) -> Option<(String, String)> {
        self.held.take()
    }

    /// Whether anything is waiting.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.held.is_some()
    }
}

/// A handle to a debounced commit. Cheap to clone.
#[derive(Clone)]
pub struct DebouncedCommit {
    pending: Rc<RefCell<PendingEdit>>,
    commit: Rc<dyn Fn(String, String)>,
    delay_ms: u64,
}

impl PartialEq for DebouncedCommit {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.pending, &other.pending)
    }
}

impl DebouncedCommit {
    /// Hold `text` for `key`; commit it `delay_ms` after the last push.
    pub fn push(&self, key: &str, text: String) {
        let (generation, other) = self.pending.borrow_mut().push(key, text);
        if let Some((k, t)) = other {
            (self.commit)(k, t);
        }
        let pending = self.pending.clone();
        let commit = self.commit.clone();
        let delay = self.delay_ms;
        spawn(async move {
            architect::sleep(architect::Duration::from_millis(delay)).await;
            let due = pending.borrow_mut().take_if(generation);
            if let Some((k, t)) = due {
                commit(k, t);
            }
        });
    }

    /// Commit whatever is held, now.
    pub fn flush(&self) {
        let due = self.pending.borrow_mut().take();
        if let Some((k, t)) = due {
            (self.commit)(k, t);
        }
    }
}

/// Debounce commits of a document body: `commit(key, text)` runs
/// `delay_ms` after typing stops, at once on [`DebouncedCommit::flush`],
/// and on unmount — so leaving the page never drops the last words. Plain
/// `Rc`s rather than signals, because the unmount flush runs while this
/// scope's signals are being torn down.
pub fn use_debounced_commit(
    delay_ms: u64,
    commit: impl Fn(String, String) + 'static,
) -> DebouncedCommit {
    let handle = use_hook(move || DebouncedCommit {
        pending: Rc::new(RefCell::new(PendingEdit::default())),
        commit: Rc::new(commit),
        delay_ms,
    });
    let on_drop = handle.clone();
    use_drop(move || on_drop.flush());
    handle
}

// ── The keyboard toolbar (phone) ────────────────────────────────────

/// `#` → `##` → `###` → none, on the caret's line.
fn cycle_heading(s: &EditorState) -> Option<TransactionSpec> {
    let doc = s.doc.to_string();
    let at = caret_of(s).min(doc.len());
    let start = doc[..at].rfind('\n').map_or(0, |n| n + 1);
    let level = doc[start..].bytes().take_while(|b| *b == b'#').count();
    let next = if level >= 3 { 0 } else { level as u8 + 1 };
    editor::commands::set_heading(s, next)
}
fn wikilink(s: &EditorState) -> Option<TransactionSpec> {
    run_here(s, CommandKind::InsertSnippet("[[]]", 2))
}
fn tag(s: &EditorState) -> Option<TransactionSpec> {
    run_here(s, CommandKind::InsertSnippet("#", 0))
}
fn inline_code(s: &EditorState) -> Option<TransactionSpec> {
    run_here(s, CommandKind::InsertSnippet("``", 1))
}
fn indent(s: &EditorState) -> Option<TransactionSpec> {
    editor::commands::tab_list_indent(s, false).or_else(|| editor::commands::indent_more(s))
}
fn outdent(s: &EditorState) -> Option<TransactionSpec> {
    editor::commands::tab_list_indent(s, true).or_else(|| editor::commands::indent_less(s))
}

/// The toolbar's actions, in Obsidian's order: links first, then the
/// marks, the blocks, and the structure.
fn toolbar_actions() -> Vec<(&'static str, fn() -> Element, editor::Command)> {
    use architect_ui::lucide_dioxus as i;
    vec![
        (
            "Link to a note",
            || rsx! { i::Brackets { size: 20 } },
            wikilink,
        ),
        (
            "Link",
            || rsx! { i::Link { size: 20 } },
            |s: &_| editor::commands::toggle_link(s),
        ),
        (
            "Bold",
            || rsx! { i::Bold { size: 20 } },
            |s: &_| editor::commands::toggle_bold(s),
        ),
        (
            "Italic",
            || rsx! { i::Italic { size: 20 } },
            |s: &_| editor::commands::toggle_italic(s),
        ),
        (
            "Heading",
            || rsx! { i::Heading { size: 20 } },
            cycle_heading,
        ),
        ("Bulleted list", || rsx! { i::List { size: 20 } }, bullets),
        (
            "Numbered list",
            || rsx! { i::ListOrdered { size: 20 } },
            numbers,
        ),
        ("Checklist", || rsx! { i::ListTodo { size: 20 } }, checklist),
        ("Indent", || rsx! { i::IndentIncrease { size: 20 } }, indent),
        (
            "Outdent",
            || rsx! { i::IndentDecrease { size: 20 } },
            outdent,
        ),
        ("Quote", || rsx! { i::TextQuote { size: 20 } }, blockquote),
        ("Code", || rsx! { i::Code { size: 20 } }, inline_code),
        ("Tag", || rsx! { i::Hash { size: 20 } }, tag),
    ]
}

/// Obsidian's bar above the phone keyboard: a scrolling pill of markdown
/// actions on the focused body, and apart from it a button that puts the
/// keyboard away. Shown while `state`'s editor (inside `scope`, a CSS
/// selector) has focus on a phone; rides the keyboard (`--kb`). Its
/// buttons never take focus (`data-keep-focus`, honoured by the shell's
/// focus rule), so the caret and the keyboard stay where they are.
#[component]
pub fn KeyboardToolbar(
    state: Signal<EditorState>,
    /// The editor's transaction sink, so a toolbar edit persists like a
    /// typed one.
    #[props(default)]
    sink: Option<Callback<editor::TransactionEvent>>,
    /// Whether the editor has focus.
    focused: bool,
) -> Element {
    let panels = try_use_context::<crate::shell::phone_gestures::PhonePanels>();
    let phone = panels.is_some_and(|p| (p.phone)());
    if !focused || !phone {
        return rsx! {};
    }
    rsx! {
        div {
            class: "fixed inset-x-0 z-50 hidden items-center gap-2 px-2 pb-2 [.kb-open_&]:flex md:hidden",
            style: "bottom: var(--kb, 0px)",
            "data-keep-focus": "true",
            "data-no-swipe": "true",
            div { class: "flex h-12 min-w-0 flex-1 items-center overflow-x-auto rounded-full bg-popover/95 px-1 shadow-[0_8px_30px_rgb(0_0_0/0.25),inset_0_0_0_1px_rgb(255_255_255/0.06)] backdrop-blur [scrollbar-width:none] [&::-webkit-scrollbar]:hidden",
                for (label, icon, run) in toolbar_actions() {
                    button {
                        key: "{label}",
                        r#type: "button",
                        class: "flex size-11 shrink-0 items-center justify-center rounded-full text-foreground/85 active:bg-foreground/10",
                        aria_label: "{label}",
                        title: "{label}",
                        onclick: move |_| {
                            let cur = state.peek().clone();
                            if let Some(spec) = run(&cur) {
                                editor::dispatch_spec(state, spec, sink);
                            }
                        },
                        {icon()}
                    }
                }
            }
            button {
                r#type: "button",
                class: "flex size-12 shrink-0 items-center justify-center rounded-full bg-popover/95 text-foreground/85 shadow-[0_8px_30px_rgb(0_0_0/0.25),inset_0_0_0_1px_rgb(255_255_255/0.06)] backdrop-blur active:bg-foreground/10",
                aria_label: "Hide keyboard",
                title: "Hide keyboard",
                onclick: move |_| {
                    let _ = document::eval("document.activeElement && document.activeElement.blur();");
                },
                architect_ui::lucide_dioxus::KeyboardOff { size: 20 }
            }
        }
    }
}

// ── The component ───────────────────────────────────────────────────

/// A markdown body, edited with the document editor. Body only — never
/// frontmatter: the host owns the record's properties and shows them in
/// its own property system.
// t[impl doc.editor.one]
#[component]
pub fn DocBody(
    /// The body, as the host currently has it. A change from outside is
    /// taken into the buffer when no local edit is outstanding (see
    /// [`reconcile`]); the echo of our own edit is ignored.
    text: String,
    /// Every change to the buffer, as the whole body.
    on_change: EventHandler<String>,
    /// Shown while the body is empty.
    #[props(default)]
    placeholder: Option<String>,
    #[props(default = true)] editable: bool,
    /// A link in the body was clicked (external links open on their own).
    #[props(default)]
    on_link_click: Option<Callback<String>>,
    /// Bind the editor to a document's `body` instead of `text`
    /// (`offline.doc.shape`): once the replica is ready its body is the
    /// buffer, edits go into it as text ops (merged character by
    /// character, offline included), and peers' edits come back in. Until
    /// it is ready the editor works from `text` and `on_change` as
    /// without one. The body is shown without its one leading separator
    /// line (`vault_doc::body::shown` — a task file's convention).
    #[props(default)]
    doc: Option<crate::doc_replica::ReplicaHandle>,
) -> Element {
    use_hook(install_palette);
    let mut state = use_signal(|| EditorState::new(text.clone()));
    let keymap = use_signal(doc_keymap);
    let palette = use_signal(|| None::<PaletteState>);
    let decorations =
        use_hook(|| editor::editor_view::DecorationSource::ptr(editor::combined_decorations));
    // What the host and the buffer last agreed on, and what we last told
    // the host — so a change is reported once and an incoming text that is
    // only our echo does nothing.
    let mut agreed = use_signal(|| text.clone());
    let mut told = use_signal(|| text.clone());
    // The document owns the buffer (bound and ready).
    let bound = use_memo(move || doc.is_some_and(|h| h.ready() && h.doc().is_some()));

    use_effect(move || {
        let buffer = state.read().doc.to_string();
        if buffer == *told.peek() {
            return;
        }
        told.set(buffer.clone());
        if *bound.peek() {
            // t[impl offline.doc.replica] — a keystroke is a text op on
            // the document's body; the replica persists and syncs it.
            if let Some(d) = doc.and_then(|h| h.doc()) {
                let body = vault_doc::doc::body_text(d.loro());
                let ops = vault_doc::body::shown_edit_ops(&body.to_string(), &buffer);
                if !ops.is_empty() {
                    vault_doc::body::apply_ops_clamped(&body, &ops);
                    vault_doc::commit_as(d.loro(), "edited in the app");
                }
            }
            return;
        }
        on_change.call(buffer);
    });

    // The document's body → the buffer: on takeover, and on every remote
    // change. Our own edits render back identical (no-op).
    use_effect(move || {
        let Some(h) = doc else { return };
        let _rev = h.revision();
        if !bound() {
            return;
        }
        let Some(d) = h.doc() else { return };
        let body = vault_doc::body::shown(&vault_doc::doc::body(d.loro())).to_owned();
        let buffer = state.peek().doc.to_string();
        if body == buffer {
            return;
        }
        told.set(body.clone());
        agreed.set(body.clone());
        if let Some((range, insert)) = minimal_replace(&buffer, &body) {
            editor::dispatch_spec(
                state,
                TransactionSpec::new()
                    .changes(Changes::replace(range, insert))
                    .annotate("origin", "remote"),
                None,
            );
        }
    });
    let unsynced = use_memo(move || doc.is_some_and(|h| h.unsynced()));

    use_effect(use_reactive!(|text| {
        if *bound.peek() {
            return; // the document is the source now, not the host's copy
        }
        let buffer = state.peek().doc.to_string();
        let verdict = reconcile(&buffer, &agreed.peek(), &text);
        match verdict {
            Incoming::InSync => agreed.set(text),
            Incoming::KeepLocal => {}
            Incoming::Apply => {
                agreed.set(text.clone());
                told.set(text.clone());
                if let Some((range, insert)) = minimal_replace(&buffer, &text) {
                    editor::dispatch_spec(
                        state,
                        TransactionSpec::new()
                            .changes(Changes::replace(range, insert))
                            .annotate("origin", "remote"),
                        None,
                    );
                } else {
                    state.set(EditorState::new(text));
                }
            }
        }
    }));

    let empty = use_memo(move || state.read().doc.is_empty());
    let placeholder = placeholder.unwrap_or_default();

    // The editor places its caret as it mounts, and on iOS a caret in a
    // contenteditable is focus — the keyboard rose over a page opened only
    // to read it. Until the person touches the body, hand focus back.
    let mut touched = use_signal(|| false);
    let mut focused = use_signal(|| false);
    use_hook(move || {
        spawn(async move {
            for _ in 0..4 {
                architect::sleep(architect::Duration::from_millis(150)).await;
                if *touched.peek() {
                    return;
                }
                let _ = document::eval(
                    "const a = document.activeElement; if (a && a.closest && a.closest('.doc-body')) a.blur();",
                );
            }
        });
    });

    rsx! {
        document::Link { rel: "stylesheet", href: editor::EDITOR_STYLE }
        document::Style { {DOC_CSS} }
        div {
            class: "{DOC_CLASS} doc-body editor-app props-collapsed relative",
            onpointerdown: move |_| touched.set(true),
            onkeydown: move |_| touched.set(true),
            onfocusin: move |_| focused.set(true),
            onfocusout: move |_| focused.set(false),
            if empty() && !placeholder.is_empty() {
                div { class: "doc-placeholder", aria_hidden: "true", "{placeholder}" }
            }
            // t[impl offline.visible] — per document: edits the server has
            // not acknowledged yet.
            if unsynced() {
                div { class: "doc-unsynced", title: "Saved on this device; syncs when the connection is back", "Not synced" }
            }
            div { class: "editor-frame editor-frame--flush",
                Editor {
                    state,
                    keymap: keymap.read().clone(),
                    decorations: decorations.clone(),
                    palette: Some(palette),
                    on_link_click,
                    editable,
                }
                CommandPalette { state, palette }
            }
            KeyboardToolbar { state, focused: focused() }
        }
    }
}

/// Linear's document typography, scoped to [`DOC_CLASS`]. Specific enough
/// (`.doc-linear .editor-root …`) to beat `editor.css` wherever it is
/// linked.
// t[impl doc.editor.typography]
pub const DOC_CSS: &str = r#"
.doc-linear {
    --doc-text: color-mix(in oklab, var(--foreground, #e3e4e6) 90%, var(--background, #000));
    --doc-muted: var(--muted-foreground, #9a9ba0);
    --doc-faint: color-mix(in oklab, var(--foreground, #e3e4e6) 6%, transparent);
    --doc-line: color-mix(in oklab, var(--foreground, #e3e4e6) 10%, transparent);
}
/* A task body sits flush in its column; a note keeps the 24px that lines
   its text up under the note title. */
.doc-body .editor-frame.editor-frame--flush { padding: 0; min-height: 0; }
.doc-body .editor-frame--flush .editor-root,
.doc-body .editor-frame--flush .editor-root.reading-mode { padding-left: 0; }
.doc-linear .editor-root {
    font-size: 15px;
    line-height: 24px;
    font-weight: 450;
    letter-spacing: -0.1px;
    color: var(--doc-text);
    min-height: 96px;
    font-feature-settings: "cv11", "ss01";
}
@media (pointer: coarse) {
    /* 16px keeps iOS from zooming the page when the editor takes focus. */
    .doc-linear .editor-root { font-size: 16px; line-height: 24px; }
}
/* A blank line between blocks is Linear's 16px paragraph gap, not a
   whole empty row. Only plain lines: a blank line inside a code block
   keeps its height. */
.doc-linear .editor-root .cm-line:not([class*="md-"]):has(> br:only-child),
.doc-linear .editor-root .cm-line:not([class*="md-"]):empty {
    min-height: 16px;
    height: 16px;
    line-height: 16px;
}
.doc-linear .editor-root .cm-line.md-h1,
.doc-linear .editor-root .cm-line.md-h2,
.doc-linear .editor-root .cm-line.md-h3,
.doc-linear .editor-root .cm-line.md-h4,
.doc-linear .editor-root .cm-line.md-h5,
.doc-linear .editor-root .cm-line.md-h6 {
    font-weight: 600;
    color: var(--foreground, #e3e4e6);
    margin-bottom: 4px;
}
/* The blank line before a heading supplies 16px of its top margin. */
.doc-linear .editor-root .cm-line.md-h1 { font-size: 22px; line-height: 29.6px; margin-top: 32px; letter-spacing: -0.2px; }
.doc-linear .editor-root .cm-line.md-h2 { font-size: 19px; line-height: 28px; margin-top: 16px; letter-spacing: -0.15px; }
.doc-linear .editor-root .cm-line.md-h3 { font-size: 16px; line-height: 24px; margin-top: 8px; }
.doc-linear .editor-root .cm-line.md-h4,
.doc-linear .editor-root .cm-line.md-h5,
.doc-linear .editor-root .cm-line.md-h6 { font-size: 15px; line-height: 24px; margin-top: 8px; }
.doc-linear .editor-root .cm-line:first-child { margin-top: 0; }
/* The markdown marks stay — they are the source — but read as margin,
   not as text. */
.doc-linear .editor-root .md-heading-marker {
    color: var(--doc-muted);
    opacity: 0.6;
    font-weight: 450;
    font-size: 0.75em;
}
.doc-linear .editor-root .cm-line.md-list-item,
.doc-linear .editor-root .cm-line.md-task { padding-left: 6px; }
.doc-linear .editor-root .md-list-marker {
    display: inline-block;
    min-width: 18px;
    color: var(--doc-muted);
    font-weight: 450;
}
.doc-linear .editor-root .md-task-checkbox {
    width: 14px;
    height: 14px;
    margin-right: 8px;
    vertical-align: -2px;
    border-radius: 4px;
    border: 1.5px solid var(--doc-muted);
    background: transparent;
}
.doc-linear .editor-root .md-task-checkbox.checked {
    background: var(--primary, #5e6ad2);
    border-color: var(--primary, #5e6ad2);
}
.doc-linear .editor-root .cm-line.md-blockquote {
    font-style: normal;
    color: var(--doc-muted);
}
.doc-linear .editor-root .cm-line.md-blockquote::before {
    width: 2px;
    border-radius: 1px;
    background: var(--doc-line);
}
.doc-linear .editor-root .cm-line.md-code-block {
    font-size: 13px;
    line-height: 20px;
    background: var(--doc-faint);
    padding: 0 12px;
}
.doc-linear .editor-root .cm-line.md-code-block:not(.md-code-block + .md-code-block) {
    border-top-left-radius: 8px;
    border-top-right-radius: 8px;
    padding-top: 8px;
}
.doc-linear .editor-root .cm-line.md-code-block:has(+ :not(.md-code-block)),
.doc-linear .editor-root .cm-line.md-code-block:last-child {
    border-bottom-left-radius: 8px;
    border-bottom-right-radius: 8px;
    padding-bottom: 8px;
}
.doc-linear .editor-root .md-code {
    font-size: 0.87em;
    padding: 1px 4px;
    border-radius: 4px;
    background: var(--doc-faint);
}
.doc-linear .editor-root .cm-line.md-hr::before { border-top-color: var(--doc-line); }

.doc-linear .doc-placeholder {
    position: absolute;
    top: 0;
    left: 0;
    font-size: 15px;
    line-height: 24px;
    color: var(--doc-muted);
    opacity: 0.7;
    pointer-events: none;
    user-select: none;
}
@media (pointer: coarse) { .doc-linear .doc-placeholder { font-size: 16px; } }
.doc-linear .doc-unsynced {
    position: absolute;
    top: -20px;
    right: 0;
    font-size: 11px;
    line-height: 16px;
    color: var(--doc-muted);
    opacity: 0.8;
    user-select: none;
}

/* ── The slash menu: Linear's popover ── */
.doc-linear .ed-menu {
    min-width: 260px;
    max-width: 340px;
    max-height: 360px;
    padding: 4px;
    border-radius: 12px;
    background: var(--popover, var(--ed-bg-elev, #161821));
    border: 1px solid var(--doc-line);
    box-shadow: 0 4px 24px rgba(0, 0, 0, 0.35), 0 1px 2px rgba(0, 0, 0, 0.2);
}
.doc-linear .ed-menu-group {
    padding: 8px 8px 4px;
    font-size: 12px;
    font-weight: 500;
    text-transform: none;
    letter-spacing: 0;
    color: var(--doc-muted);
}
.doc-linear .ed-menu-row,
.doc-linear .ed-menu-row:has(.ed-menu-desc) {
    grid-template-columns: 22px 1fr;
    align-items: center;
    gap: 8px;
    min-height: 32px;
    padding: 0 8px;
    border-radius: 8px;
    cursor: pointer;
    font-size: 13px;
    font-weight: 500;
}
.doc-linear .ed-menu-row:hover { background: var(--doc-faint); }
.doc-linear .ed-menu-row.selected {
    background: color-mix(in oklab, var(--foreground, #e3e4e6) 9%, transparent);
    box-shadow: none;
}
.doc-linear .ed-menu-icon,
.doc-linear .ed-menu-row:has(.ed-menu-desc) .ed-menu-icon,
.doc-linear .ed-menu-row.selected .ed-menu-icon {
    width: 22px;
    height: 20px;
    margin: 0;
    padding: 0;
    border: none;
    background: none;
    font-size: 11px;
    color: var(--doc-muted);
}
.doc-linear .ed-menu-body {
    flex-direction: row;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
}
.doc-linear .ed-menu-label { color: var(--foreground, #e3e4e6); white-space: nowrap; }
.doc-linear .ed-menu-desc {
    margin: 0;
    font-size: 12px;
    font-weight: 450;
    color: var(--doc-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 55%;
}
@media (pointer: coarse) {
    .doc-linear .ed-menu-row { min-height: 44px; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    // t[verify doc.editor.slash]
    #[test]
    fn the_menu_opens_on_linears_groups_in_linears_order() {
        let cmds = doc_commands();
        let labels: Vec<_> = cmds.iter().take(15).map(|c| (c.group, c.label)).collect();
        assert_eq!(
            labels,
            vec![
                ("Text", "Heading 1"),
                ("Text", "Heading 2"),
                ("Text", "Heading 3"),
                ("Text", "Bulleted list"),
                ("Text", "Numbered list"),
                ("Text", "Checklist"),
                ("Insert", "Image"),
                ("Insert", "Attach files"),
                ("Insert", "Code block"),
                ("Insert", "Diagram"),
                ("Insert", "Collapsible section"),
                ("Insert", "Blockquote"),
                ("Insert", "Callout"),
                ("Insert", "Divider"),
                ("Insert", "Table"),
            ]
        );
        let shortcut = |l: &str| cmds.iter().find(|c| c.label == l).map(|c| c.desc);
        assert_eq!(shortcut("Heading 1"), Some("⌘⌥1"));
        assert_eq!(shortcut("Checklist"), Some("⌘⇧7"));
        assert_eq!(shortcut("Code block"), Some("⌘⇧\\"));
        assert_eq!(shortcut("Blockquote"), Some("⌥⇧."));
    }

    #[test]
    fn nothing_is_listed_twice_and_the_wiki_extras_stay() {
        let cmds = doc_commands();
        let mut labels: Vec<_> = cmds.iter().map(|c| (c.group, c.label)).collect();
        let n = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), n, "a command appears twice");
        for kept in ["Readings", "Wikilink", "Math block", "Heading 4", "Warning"] {
            assert!(cmds.iter().any(|c| c.label == kept), "{kept} was dropped");
        }
        // The group headers stay contiguous — the menu prints a header
        // whenever the group changes, so a split group prints twice.
        let mut seen: Vec<&str> = Vec::new();
        for c in &cmds {
            if seen.last() != Some(&c.group) {
                assert!(!seen.contains(&c.group), "group {} split", c.group);
                seen.push(c.group);
            }
        }
    }

    fn apply(doc: &str, caret: usize, kind: CommandKind) -> String {
        let mut s = EditorState::new(doc.to_owned());
        s.selection = Selection::caret(caret);
        let spec = run_command(&s, caret..caret, kind).expect("a spec");
        s.update(spec).doc.to_string()
    }

    #[test]
    fn the_inserts_write_plain_markdown() {
        assert_eq!(apply("", 0, CODE_BLOCK), "```\n\n```\n");
        assert_eq!(apply("", 0, DIAGRAM), "```mermaid\n\n```\n");
        assert_eq!(apply("Notes", 5, DIVIDER), "Notes\n\n---\n");
        assert_eq!(apply("", 0, COLLAPSIBLE), "> [!note]- Section title\n> \n");
        assert_eq!(apply("", 0, IMAGE), "![](https://)");
        assert_eq!(
            apply("Mix", 3, CommandKind::SetList(ListKind::Task)),
            "- [ ] Mix"
        );
    }

    // t[verify doc.editor.shortcuts]
    #[test]
    fn the_shortcuts_reach_the_same_commands() {
        let km = doc_keymap();
        let press = |key: &str, doc: &str| {
            let mut s = EditorState::new(doc.to_owned());
            s.selection = Selection::caret(doc.len());
            km.dispatch(&editor::KeySpec::from(key), &s)
                .map(|spec| s.update(spec).doc.to_string())
        };
        // A Mac reports ⌘⌥1 as `¡` and ⌘⇧8 as `*`; both spellings bind.
        assert_eq!(press("Meta-Alt-¡", "Plan").as_deref(), Some("# Plan"));
        assert_eq!(press("Meta-Alt-1", "Plan").as_deref(), Some("# Plan"));
        assert_eq!(press("Meta-Shift-*", "Mix").as_deref(), Some("- Mix"));
        assert_eq!(press("Ctrl-Shift-9", "Mix").as_deref(), Some("1. Mix"));
        assert_eq!(press("Meta-Shift-&", "Mix").as_deref(), Some("- [ ] Mix"));
        assert_eq!(press("Alt-Shift-˘", "Said").as_deref(), Some("> Said"));
        // And the standard bindings are still there.
        assert!(press("Meta-a", "x").is_some());
    }

    // t[verify doc.editor.body-only]
    #[test]
    fn an_echo_is_ignored_and_a_remote_change_waits_for_local_edits() {
        assert_eq!(reconcile("a", "", "a"), Incoming::InSync);
        assert_eq!(reconcile("a", "a", "b"), Incoming::Apply);
        assert_eq!(reconcile("a typed", "a", "b"), Incoming::KeepLocal);
    }

    #[test]
    fn a_remote_change_lands_as_the_smallest_edit() {
        assert_eq!(minimal_replace("same", "same"), None);
        assert_eq!(
            minimal_replace("# Plan\nmix", "# Plan\nmaster"),
            Some((8..10, "aster".to_owned()))
        );
        assert_eq!(minimal_replace("aaa", "aaaa"), Some((3..3, "a".to_owned())));
        assert_eq!(minimal_replace("abc", ""), Some((0..3, String::new())));
        // Multibyte: boundaries stay on chars.
        let (r, s) = minimal_replace("café ✓", "cafe ✓").expect("differs");
        let mut out = "café ✓".to_owned();
        out.replace_range(r, &s);
        assert_eq!(out, "cafe ✓");
    }

    #[test]
    fn only_the_newest_timer_commits_and_a_switch_commits_the_old_document() {
        let mut p = PendingEdit::default();
        let (g1, none) = p.push("a", "one".into());
        assert!(none.is_none());
        let (g2, none) = p.push("a", "one two".into());
        assert!(none.is_none());
        assert_eq!(p.take_if(g1), None, "a stale timer does nothing");
        assert_eq!(p.take_if(g2), Some(("a".into(), "one two".into())));
        assert!(!p.is_pending());

        p.push("a", "draft".into());
        let (_, flushed) = p.push("b", "other".into());
        assert_eq!(flushed, Some(("a".into(), "draft".into())));
        assert_eq!(p.take(), Some(("b".into(), "other".into())));
        assert_eq!(p.take(), None);
    }
}
