//! Dumb Dioxus components for the threads feature.
//!
//! [`ConversationsPanel`] renders a topic list + the selected thread's
//! messages + composers. Props in, `EventHandler` out — the hosting page
//! owns the data + fetching (per the dumb-component rule).

use dioxus::prelude::*;
use architect_ui::prelude::*;
use threads_proto::{Message, Thread};
use uuid::Uuid;

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
    let mut new_title = use_signal(String::new);
    let mut draft = use_signal(String::new);

    rsx! {
        div { class: "flex flex-col gap-3",
            Heading { level: HeadingLevel::H2, "Conversations" }

            // New-topic composer.
            div { class: "flex items-center gap-2",
                Input { value: new_title, placeholder: "New topic…", on_change: move |_| {} }
                Button {
                    variant: ButtonVariant::Secondary,
                    size: ButtonSize::Small,
                    on_click: move |_| {
                        let t = new_title.read().trim().to_string();
                        if !t.is_empty() {
                            on_new_thread.call(t);
                            new_title.set(String::new());
                        }
                    },
                    "Add topic"
                }
            }

            if threads.is_empty() {
                Text { variant: TextVariant::Muted, class: "text-sm", "No conversations yet." }
            }

            // Topic list.
            div { class: "flex flex-col gap-1",
                for t in threads.iter().cloned() {
                    {
                        let id = t.id;
                        let row_class = if selected == Some(id) {
                            "cursor-pointer rounded-md bg-accent px-2 py-1 text-sm"
                        } else {
                            "cursor-pointer rounded-md px-2 py-1 text-sm hover:bg-muted"
                        };
                        rsx! {
                            div {
                                key: "{id}",
                                class: row_class,
                                onclick: move |_| on_select.call(id),
                                span { "{t.title}" }
                                if t.resolved {
                                    span { class: "ml-2 text-xs text-muted-foreground", "(resolved)" }
                                }
                            }
                        }
                    }
                }
            }

            // Selected thread's messages + reply composer.
            if selected.is_some() {
                div { class: "mt-1 flex flex-col gap-2 border-t border-border pt-2",
                    if messages.is_empty() {
                        Text { variant: TextVariant::Muted, class: "text-sm", "No messages yet." }
                    }
                    for m in messages.iter().cloned() {
                        div { key: "{m.id}", class: "flex flex-col",
                            span { class: "text-xs text-muted-foreground", "{m.author_label}" }
                            span { class: "text-sm", "{m.body}" }
                        }
                    }
                    div { class: "flex items-center gap-2",
                        Input { value: draft, placeholder: "Message…", on_change: move |_| {} }
                        Button {
                            variant: ButtonVariant::Primary,
                            size: ButtonSize::Small,
                            on_click: move |_| {
                                let b = draft.read().trim().to_string();
                                if !b.is_empty() {
                                    on_post.call(b);
                                    draft.set(String::new());
                                }
                            },
                            "Send"
                        }
                    }
                }
            }
        }
    }
}
