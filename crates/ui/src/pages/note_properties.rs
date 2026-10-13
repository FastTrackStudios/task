//! Right-sidebar frontmatter **properties** panel.
//!
//! Replaces the editor's inline `.md-properties` widget: the same YAML
//! frontmatter, rendered as editable rows in the right sidebar (a tab
//! next to Links). Because we edit the **focused note's live editor
//! doc** — published via [`FocusedDoc`] and mutated through
//! [`editor::dispatch_spec`] with the note's own transaction sink —
//! edits round-trip to collab + autosave exactly like inline edits do.
//!
//! Parsing + serialization reuse the editor's own
//! [`parse_frontmatter`] / [`serialize_property`], so the byte-range
//! contract (replace one property without touching the rest of the
//! block) matches the inline widget precisely. Each edit re-parses the
//! current doc and locates the property by key, so a concurrent edit
//! can never make a captured byte-range go stale.

use architect_ui::lucide_dioxus::{Plus, X};
use dioxus::prelude::*;
use editor::markdown::{PropValue, parse_frontmatter, serialize_property};
use editor::{Changes, EditorState, TransactionEvent, TransactionSpec, dispatch_spec};

/// Handle to the focused note's editor doc. The focused
/// [`NoteView`](crate::pages::note_view) publishes this into context;
/// the sidebar panel consumes it. `None` when no note is focused.
#[derive(Clone, Copy, PartialEq)]
pub struct FocusedDoc {
    /// Per-`NoteView` claim id — identity used so a pane only clears
    /// the context when it is still the current holder (split-view
    /// focus swaps + keyed-remount races would otherwise drop the
    /// newly-focused doc).
    pub claim: u64,
    /// The focused note's editor state (its document buffer).
    pub state: Signal<EditorState>,
    /// The note's `on_transaction` sink — routes edits to the CRDT
    /// host so sidebar property edits replicate + autosave.
    pub on_transaction: Callback<TransactionEvent>,
    /// Whether this person may write the note. `false` shows the
    /// properties without the controls that change them.
    pub writable: bool,
}

/// A process-unique claim id for a `NoteView` instance.
pub fn next_claim() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(1);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Dispatch a document edit against the focused doc through its own
/// transaction sink (same path as an in-editor edit).
fn dispatch(doc: FocusedDoc, changes: Changes) {
    if !doc.writable {
        return;
    }
    dispatch_spec(
        doc.state,
        TransactionSpec::new()
            .changes(changes)
            .annotate("origin", "prop-edit"),
        Some(doc.on_transaction),
    );
}

/// Replace property `key`'s value, re-parsing the live doc first so
/// the byte-range is always current.
fn set_prop(doc: FocusedDoc, key: &str, value: PropValue) {
    let text = doc.state.read().doc.to_string();
    let Some(fm) = parse_frontmatter(&text) else {
        return;
    };
    let Some(prop) = fm.props.iter().find(|p| p.key == key) else {
        return;
    };
    let new_text = serialize_property(key, &value);
    dispatch(doc, Changes::replace(prop.range.clone(), new_text));
}

/// Remove property `key` entirely.
fn delete_prop(doc: FocusedDoc, key: &str) {
    let text = doc.state.read().doc.to_string();
    let Some(fm) = parse_frontmatter(&text) else {
        return;
    };
    let Some(prop) = fm.props.iter().find(|p| p.key == key) else {
        return;
    };
    dispatch(doc, Changes::delete(prop.range.clone()));
}

/// Add a new `key: value` property. Inserts before the closing `---`
/// when frontmatter exists; otherwise seeds a fresh block at the top
/// of the document.
fn add_prop(doc: FocusedDoc, key: &str, value: &str) {
    let key = key.trim();
    if key.is_empty() {
        return;
    }
    let text = doc.state.read().doc.to_string();
    let serialized = serialize_property(key, &PropValue::Text(value.trim().to_owned()));
    match parse_frontmatter(&text) {
        Some(fm) => {
            // Don't duplicate an existing key — update it instead.
            if fm.props.iter().any(|p| p.key == key) {
                set_prop(doc, key, PropValue::Text(value.trim().to_owned()));
                return;
            }
            dispatch(doc, Changes::insert(fm.closer.start, serialized));
        }
        None => {
            let block = format!("---\n{serialized}---\n");
            dispatch(doc, Changes::insert(0, block));
        }
    }
}

/// Infer the new [`PropValue`] from a text-field edit, preserving the
/// original scalar type when the input still fits it (a number field
/// stays a number, a date stays a date), else falling back to text.
fn value_from_text(prev: &PropValue, raw: &str) -> PropValue {
    let raw = raw.trim();
    match prev {
        PropValue::Number(_) => raw
            .parse::<f64>()
            .map(PropValue::Number)
            .unwrap_or_else(|_| PropValue::Text(raw.to_owned())),
        PropValue::Date(_) if is_iso_date(raw) => PropValue::Date(raw.to_owned()),
        _ if raw.is_empty() => PropValue::Empty,
        _ => PropValue::Text(raw.to_owned()),
    }
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && s.chars().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// Display text for a scalar value shown in a text input.
fn scalar_text(v: &PropValue) -> String {
    match v {
        PropValue::Text(s) => s.clone(),
        PropValue::Number(n) => {
            if n.fract() == 0.0 {
                format!("{}", *n as i64)
            } else {
                format!("{n}")
            }
        }
        PropValue::Date(s) => s.clone(),
        PropValue::List(items) => items.join(", "),
        PropValue::Bool(b) => b.to_string(),
        PropValue::Empty => String::new(),
    }
}

/// A value that reads as a chip and edits in place: borderless until
/// hovered or focused, 13px medium — the task property column's look.
const VALUE_INPUT: &str = "h-8 w-full min-w-0 rounded-md border border-transparent bg-transparent px-2 text-[13px] font-medium text-foreground outline-none transition-colors placeholder:text-muted-foreground hover:bg-accent focus-visible:border-border focus-visible:bg-accent/40 disabled:hover:bg-transparent md:h-7";

/// The add-property key field: the label column's look, editable.
const KEY_INPUT: &str = "h-8 w-20 shrink-0 rounded-md border border-transparent bg-transparent px-0 text-[13px] font-medium text-muted-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:text-foreground md:h-7";

/// The focused note's properties in the right sidebar, laid out the way a
/// task's are: the shared property row (name left, chip right).
#[component]
pub fn NoteProperties() -> Element {
    let focused = use_context::<Signal<Option<FocusedDoc>>>();
    let Some(doc) = *focused.read() else {
        return rsx! {
            div { class: "px-3 py-4 text-[13px] text-muted-foreground", "Open a note to see its properties." }
        };
    };
    rsx! {
        div { class: "px-3 py-3", PropertiesPanel { doc } }
    }
}

/// The property rows for one note's frontmatter, plus "Add property".
/// The sidebar shows it on a desk; on a phone it opens under the chip
/// row ([`NotePropertyChips`]).
// t[impl doc.properties.shared]
#[component]
pub fn PropertiesPanel(doc: FocusedDoc) -> Element {
    let mut new_key = use_signal(String::new);
    let mut new_val = use_signal(String::new);

    // Reactive read — re-renders whenever the doc changes (typing,
    // collab, or our own edits).
    let text = doc.state.read().doc.to_string();
    let fm = parse_frontmatter(&text);
    let props = fm.as_ref().map(|f| f.props.clone()).unwrap_or_default();
    let mut add = move || {
        add_prop(doc, &new_key.peek().clone(), &new_val.peek().clone());
        new_key.set(String::new());
        new_val.set(String::new());
    };

    rsx! {
        div { class: "flex flex-col gap-0.5",
            if props.is_empty() {
                div { class: "pb-2 text-[13px] text-muted-foreground", "No properties yet." }
            }
            for prop in props.iter().cloned() {
                {
                    let key = prop.key.clone();
                    let del_key = key.clone();
                    rsx! {
                        div { key: "{prop.key}", class: "group relative",
                            task_ui::PropertyRow { label: key.clone(),
                                div { class: "flex items-center gap-1",
                                    div { class: "min-w-0 flex-1", {value_field(doc, &key, &prop.value)} }
                                    if doc.writable {
                                        button {
                                            r#type: "button",
                                            class: "flex size-6 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 transition-opacity hover:bg-accent hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100",
                                            title: "Remove property",
                                            aria_label: "Remove {del_key}",
                                            onclick: move |_| delete_prop(doc, &del_key),
                                            X { class: "size-3.5" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if doc.writable {
                div { class: "mt-1 flex min-h-8 items-center gap-3",
                    input {
                        class: KEY_INPUT,
                        aria_label: "New property name",
                        placeholder: "Add property",
                        value: "{new_key}",
                        oninput: move |e| new_key.set(e.value()),
                    }
                    input {
                        class: VALUE_INPUT,
                        aria_label: "New property value",
                        placeholder: "Value",
                        value: "{new_val}",
                        oninput: move |e| new_val.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter {
                                add();
                            }
                        },
                    }
                    button {
                        r#type: "button",
                        class: "flex size-6 shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground",
                        title: "Add property",
                        aria_label: "Add property",
                        onclick: move |_| add(),
                        Plus { class: "size-3.5" }
                    }
                }
            }
        }
    }
}

/// What a property's chip says on a phone: the value, or the name when
/// there is nothing to show (a `true` flag reads as its name).
fn chip_text(key: &str, value: &PropValue) -> Option<String> {
    match value {
        PropValue::Bool(true) => Some(key.to_owned()),
        PropValue::Bool(false) | PropValue::Empty => None,
        PropValue::List(items) if items.is_empty() => None,
        other => Some(scalar_text(other)).filter(|s| !s.trim().is_empty()),
    }
}

/// A note's properties on a phone, the way a task's sit under its title:
/// one chip per property with a value, and a "Properties" chip that opens
/// the full panel in place.
#[component]
pub fn NotePropertyChips(doc: FocusedDoc) -> Element {
    let mut open = use_signal(|| false);
    let text = doc.state.read().doc.to_string();
    let props = parse_frontmatter(&text)
        .map(|f| f.props)
        .unwrap_or_default();
    let chips: Vec<(String, String)> = props
        .iter()
        .filter_map(|p| chip_text(&p.key, &p.value).map(|v| (p.key.clone(), v)))
        .collect();
    if chips.is_empty() && !doc.writable {
        return rsx! {};
    }
    rsx! {
        div { class: "flex flex-col gap-2",
            div { class: "flex flex-wrap items-center gap-1.5",
                for (key, value) in chips {
                    button {
                        key: "{key}",
                        r#type: "button",
                        class: task_ui::CHIP_CLASS,
                        title: "{key}",
                        onclick: move |_| open.toggle(),
                        span { class: "text-muted-foreground", "{key}" }
                        span { class: "truncate", "{value}" }
                    }
                }
                button {
                    r#type: "button",
                    class: task_ui::CHIP_QUIET_CLASS,
                    aria_expanded: "{open}",
                    onclick: move |_| open.toggle(),
                    if open() { "Done" } else { "Properties" }
                }
            }
            if open() {
                div { class: "rounded-xl border border-border/60 px-3 py-2", PropertiesPanel { doc } }
            }
        }
    }
}

/// The value editor for one property — a checkbox for booleans, else a
/// text field committing `onchange` (blur/Enter) so per-keystroke
/// re-renders don't fight the caret.
fn value_field(doc: FocusedDoc, key: &str, value: &PropValue) -> Element {
    match value {
        PropValue::Bool(b) => {
            let key = key.to_owned();
            let checked = *b;
            rsx! {
                input {
                    r#type: "checkbox",
                    disabled: !doc.writable,
                    class: "ml-2 size-4 accent-primary",
                    checked,
                    onchange: move |e| {
                        set_prop(doc, &key, PropValue::Bool(e.value() == "true"));
                    },
                }
            }
        }
        other => {
            let key = key.to_owned();
            let prev = other.clone();
            let shown = scalar_text(other);
            let is_list = matches!(other, PropValue::List(_));
            rsx! {
                input {
                    class: VALUE_INPUT,
                    placeholder: "Empty",
                    value: "{shown}",
                    disabled: !doc.writable,
                    onchange: move |e| {
                        let raw = e.value();
                        let new_value = if is_list {
                            PropValue::List(
                                raw.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect(),
                            )
                        } else {
                            value_from_text(&prev, &raw)
                        };
                        set_prop(doc, &key, new_value);
                    },
                }
            }
        }
    }
}
