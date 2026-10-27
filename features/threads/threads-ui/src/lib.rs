//! Dumb Dioxus components for the threads feature.
//!
//! [`ConversationsPanel`] renders a topic list + the selected thread's
//! messages + composers. Props in, `EventHandler` out — the hosting page
//! owns the data + fetching (per the dumb-component rule).
//!
//! # Hosting outside Task (Blitz included)
//!
//! The panel renders under Blitz (dioxus-native: Stylo, Taffy, Vello) as
//! well as in a browser: no `placeholder` (the hint is drawn), no
//! `onchange`, no `<select>`, nothing fixed or sticky. A host outside
//! this repo mounts the sheets as static strings — Blitz loads no
//! external stylesheet — with [`Styles`], or [`UTILITIES_CSS`] alone when
//! it already carries architect-ui's `THEME_CSS` and `UTILITIES_CSS`.
//! Colours come from architect-ui's token custom properties
//! (`--background`, `--foreground`, `--muted-foreground`, `--border`,
//! `--primary`, `--accent`, …): set them on any ancestor to theme the
//! panel.

use architect_ui::prelude::*;
use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use threads_proto::{Message, Thread};
use uuid::Uuid;

/// The compiled Tailwind utilities this crate's components use (theme
/// variables + utilities, no preflight). Rebuild with `just ui-sheets`.
pub const UTILITIES_CSS: &str = include_str!("../assets/utilities.css");

/// The sheets the panel needs — architect-ui's tokens and utilities,
/// then this crate's — as `<style>` elements (which also work in a
/// headless document with no `<head>`). Mount it once, above the panel.
#[component]
pub fn Styles() -> Element {
    rsx! {
        style { {architect_ui::THEME_CSS} }
        style { {architect_ui::UTILITIES_CSS} }
        style { {UTILITIES_CSS} }
    }
}

/// Topic list + selected thread's messages + composers, for one anchored
/// entity (a task or project). Fully controlled by the caller.
#[component]
pub fn ConversationsPanel(
    /// Threads anchored to the host entity (newest first).
    threads: Vec<Thread>,
    /// Messages of the [`selected`] thread, oldest first.
    messages: Vec<Message>,
    /// Currently-open thread, if any.
    selected: Option<Uuid>,
    /// User picked a thread to view.
    on_select: EventHandler<Uuid>,
    /// User submitted a new topic (the title).
    on_new_thread: EventHandler<String>,
    /// User posted a message into the selected thread (the body).
    on_post: EventHandler<String>,
) -> Element {
    let open = selected.and_then(|id| threads.iter().find(|t| t.id == id).cloned());

    rsx! {
        div { class: "flex min-h-0 min-w-0 flex-col gap-3 text-foreground",
            div { class: "flex items-center gap-2",
                Heading { level: HeadingLevel::H2, class: "text-base font-semibold", "Conversations" }
                if !threads.is_empty() {
                    span { class: "text-xs tabular-nums text-muted-foreground", "{threads.len()}" }
                }
            }

            Composer {
                hint: "New topic…",
                action: "Add topic",
                primary: false,
                on_submit: on_new_thread,
            }

            if threads.is_empty() {
                p { class: "px-1 text-sm text-muted-foreground", "No conversations yet." }
            }

            // Topic list.
            div { class: "flex flex-col gap-0.5",
                for t in threads.iter().cloned() {
                    TopicRow {
                        key: "{t.id}",
                        selected: selected == Some(t.id),
                        thread: t,
                        on_select,
                    }
                }
            }

            // The open thread: its messages, then the reply composer.
            if let Some(thread) = open {
                div { class: "flex min-w-0 flex-col gap-3 border-t border-border pt-3",
                    div { class: "flex min-w-0 items-center gap-2",
                        span { class: "min-w-0 flex-1 text-sm font-medium text-foreground break-words", "{thread.title}" }
                        if thread.resolved {
                            span { class: "shrink-0 rounded-full bg-emerald-500/15 px-2 py-0.5 text-xs text-foreground", "Resolved" }
                        }
                    }
                    if messages.is_empty() {
                        p { class: "px-1 text-sm text-muted-foreground", "No messages yet." }
                    }
                    div { class: "flex min-w-0 flex-col gap-3",
                        for m in messages.iter().cloned() {
                            MessageRow { key: "{m.id}", message: m }
                        }
                    }
                    Composer {
                        hint: "Reply…",
                        action: "Send",
                        primary: true,
                        on_submit: on_post,
                    }
                }
            }
        }
    }
}

/// One topic in the list: title, its kind when it isn't plain
/// discussion, and whether it is settled.
#[component]
fn TopicRow(thread: Thread, selected: bool, on_select: EventHandler<Uuid>) -> Element {
    let id = thread.id;
    let kind =
        (thread.kind != "discussion" && !thread.kind.is_empty()).then(|| thread.kind.clone());
    rsx! {
        div {
            class: if selected {
                "flex min-h-9 min-w-0 cursor-pointer items-center gap-2 rounded-md bg-accent px-2 py-1.5 text-sm text-accent-foreground"
            } else {
                "flex min-h-9 min-w-0 cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-sm text-foreground hover:bg-accent/60"
            },
            role: "button",
            aria_pressed: selected.to_string(),
            onclick: move |_| on_select.call(id),
            span { class: if thread.resolved { "size-2 shrink-0 rounded-full bg-emerald-500" } else { "size-2 shrink-0 rounded-full bg-primary" } }
            span { class: "min-w-0 flex-1 overflow-hidden whitespace-nowrap", "{thread.title}" }
            if let Some(kind) = kind {
                span { class: "shrink-0 rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground", "{kind}" }
            }
            if thread.resolved {
                span { class: "shrink-0 text-xs text-muted-foreground", "resolved" }
            }
        }
    }
}

/// One message: who, when, what — the body wraps, never clips.
#[component]
fn MessageRow(message: Message) -> Element {
    rsx! {
        div { class: "flex min-w-0 items-start gap-2.5",
            span { class: "flex size-7 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-semibold text-foreground",
                "{initials(&message.author_label)}"
            }
            div { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                div { class: "flex min-w-0 items-baseline gap-2",
                    span { class: "text-sm font-medium text-foreground", "{message.author_label}" }
                    span { class: "text-xs tabular-nums text-muted-foreground", "{when(message.posted_at)}" }
                }
                p { class: "text-sm leading-relaxed text-foreground whitespace-pre-wrap break-words", "{message.body}" }
            }
        }
    }
}

/// A one-line composer: a text field with a drawn hint (Blitz has no
/// `placeholder`), and its button. Enter submits through the form.
#[component]
fn Composer(
    hint: &'static str,
    action: &'static str,
    primary: bool,
    on_submit: EventHandler<String>,
) -> Element {
    let mut draft = use_signal(String::new);
    let mut submit = move || {
        let text = draft.read().trim().to_owned();
        if !text.is_empty() {
            on_submit.call(text);
            draft.set(String::new());
        }
    };
    rsx! {
        form {
            class: "flex min-w-0 items-center gap-2",
            onsubmit: move |e: FormEvent| {
                e.prevent_default();
                submit();
            },
            div { class: "relative min-w-0 flex-1",
                input {
                    r#type: "text",
                    aria_label: "{hint}",
                    value: "{draft}",
                    class: "h-9 w-full min-w-0 rounded-lg border border-input bg-input/30 px-3 text-sm text-foreground outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/50",
                    oninput: move |e| draft.set(e.value()),
                }
                if draft.read().is_empty() {
                    span { class: "pointer-events-none absolute inset-y-0 left-3 flex items-center text-sm text-muted-foreground",
                        "{hint}"
                    }
                }
            }
            Button {
                variant: if primary { ButtonVariant::Primary } else { ButtonVariant::Secondary },
                size: ButtonSize::Small,
                on_click: move |_| submit(),
                "{action}"
            }
        }
    }
}

/// Two-letter initials for the avatar.
fn initials(name: &str) -> String {
    let mut words = name.split_whitespace();
    let first = words.next().and_then(|w| w.chars().next());
    let last = words.last().and_then(|w| w.chars().next());
    match (first, last) {
        (Some(a), Some(b)) => format!("{a}{b}").to_uppercase(),
        (Some(_), None) => name.chars().take(2).collect::<String>().to_uppercase(),
        _ => "?".to_owned(),
    }
}

/// "Oct 24, 14:02" in the viewer's zone.
fn when(at: DateTime<Utc>) -> String {
    at.with_timezone(&chrono::Local)
        .format("%b %-d, %H:%M")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::initials;

    #[test]
    fn initials_take_the_first_and_last_words() {
        assert_eq!(initials("Alice Liddell"), "AL");
        assert_eq!(initials("Sam"), "SA");
        assert_eq!(initials("  "), "?");
    }
}
