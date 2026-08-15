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
#[derive(Clone, Copy)]
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

const INPUT: &str = "h-8 w-full rounded-md border border-input bg-input/30 px-2 text-sm text-foreground focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/40";

#[component]
pub fn NoteProperties() -> Element {
    let focused = use_context::<Signal<Option<FocusedDoc>>>();
    let mut new_key = use_signal(String::new);
    let mut new_val = use_signal(String::new);

    let Some(doc) = *focused.read() else {
        return rsx! {
            div { class: "px-3 py-4 text-sm text-muted-foreground", "Open a note to see its properties." }
        };
    };

    // Reactive read — re-renders whenever the focused doc changes
    // (typing, collab, or our own edits).
    let text = doc.state.read().doc.to_string();
    let fm = parse_frontmatter(&text);
    let props = fm.as_ref().map(|f| f.props.clone()).unwrap_or_default();

    rsx! {
        div { class: "flex flex-col gap-1 px-3 py-3",
            if props.is_empty() {
                div { class: "px-1 pb-2 text-sm text-muted-foreground",
                    "No properties yet."
                }
            }
            for prop in props.iter().cloned() {
                {
                    let key = prop.key.clone();
                    let del_key = key.clone();
                    rsx! {
                        div { key: "{prop.key}", class: "grid grid-cols-[7rem_1fr_auto] items-center gap-1.5 py-0.5",
                            span {
                                class: "truncate text-xs font-medium uppercase tracking-wide text-muted-foreground",
                                title: "{prop.key}",
                                "{prop.key}"
                            }
                            {value_field(doc, &key, &prop.value)}
                            button {
                                class: "flex size-6 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 transition-opacity hover:bg-accent hover:text-foreground group-hover:opacity-100",
                                title: "Remove property",
                                onclick: move |_| delete_prop(doc, &del_key),
                                X { class: "size-3.5" }
                            }
                        }
                    }
                }
            }
            // Add-property row.
            div { class: "mt-2 grid grid-cols-[7rem_1fr_auto] items-center gap-1.5 border-t border-border/50 pt-3",
                input {
                    class: INPUT,
                    placeholder: "key",
                    value: "{new_key}",
                    oninput: move |e| new_key.set(e.value()),
                }
                input {
                    class: INPUT,
                    placeholder: "value",
                    value: "{new_val}",
                    oninput: move |e| new_val.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            add_prop(doc, &new_key.peek().clone(), &new_val.peek().clone());
                            new_key.set(String::new());
                            new_val.set(String::new());
                        }
                    },
                }
                button {
                    class: "flex size-6 shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground",
                    title: "Add property",
                    onclick: move |_| {
                        add_prop(doc, &new_key.peek().clone(), &new_val.peek().clone());
                        new_key.set(String::new());
                        new_val.set(String::new());
                    },
                    Plus { class: "size-3.5" }
                }
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
                    class: "size-4 justify-self-start accent-primary",
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
                    class: INPUT,
                    value: "{shown}",
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
