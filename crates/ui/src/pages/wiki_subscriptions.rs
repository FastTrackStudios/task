//! The Subscriptions tab on `/wiki` — what this org holds, and the
//! four things a person does with it.
//!
//! A subscription and its local copy are different facts, and the
//! panel keeps them visibly different: a source can be held with
//! nothing on disk (subscribed, never refreshed), and a copy can carry
//! work upstream has never seen. Collapsing the two into one "synced"
//! badge would hide exactly the state a person needs before they drop
//! something.
//!
//! Three things this deliberately shows rather than tidies away:
//!
//! - **Declined core subscriptions stay listed**, greyed, with a way
//!   back. One that vanished could never be turned on again.
//! - **A refusal to unsubscribe is shown verbatim.** The server
//!   refuses when a copy has unpushed local work and says how much;
//!   swallowing that and offering a generic "are you sure?" would
//!   throw away the only number that makes the decision.
//! - **Resources are marked read-only.** Editability follows the kind,
//!   and a reader should know before they type, not when a push is
//!   refused.

use architect_ui::prelude::*;
use dioxus::prelude::*;

use crate::orgs::{OrgMeta, OrgSelection, selected_slugs};

#[component]
pub fn SubscriptionsPanel() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    let org = use_memo(move || {
        selected_slugs(&selection.read(), &org_list.read())
            .first()
            .cloned()
    });

    let mut held = use_resource(move || async move {
        let Some(slug) = org() else {
            return Err("no organization selected".to_string());
        };
        crate::feeds::fetch_subscriptions(&slug).await
    });

    // The last thing an action said. Errors here are the point rather
    // than an edge case — the unsubscribe refusal is a designed
    // outcome, not a failure.
    let mut notice = use_signal(String::new);
    let mut busy = use_signal(String::new);

    // Add-a-subscription form.
    let mut new_ref = use_signal(String::new);
    let mut as_resource = use_signal(|| false);

    // An `EventHandler` rather than a closure passed by `&mut`: rsx
    // event handlers must be `'static`, and a borrowed `FnMut` cannot
    // escape into one. `EventHandler` is `Copy`, so each row can take
    // it by value.
    let act = EventHandler::new(move |(qualified, what): (String, Action)| {
        let Some(slug) = org() else { return };
        busy.set(qualified.clone());
        notice.set(String::new());
        spawn(async move {
            let outcome = match what {
                Action::Refresh => crate::feeds::refresh_subscription(&slug, &qualified)
                    .await
                    .map(|r| {
                        let mut said = format!(
                            "{}: pulled {}, {} already current",
                            qualified, r.pulled, r.in_sync
                        );
                        if !r.local_only.is_empty() {
                            said.push_str(&format!(
                                " — {} local page(s) upstream has not seen",
                                r.local_only.len()
                            ));
                        }
                        if !r.conflicted.is_empty() {
                            said.push_str(&format!(
                                ", {} conflicted and left for you",
                                r.conflicted.len()
                            ));
                        }
                        said
                    }),
                Action::Unsubscribe { force } => {
                    crate::feeds::unsubscribe_from(&slug, &qualified, force)
                        .await
                        .map(|()| format!("{qualified}: unsubscribed"))
                }
            };
            match outcome {
                Ok(said) => notice.set(said),
                Err(e) => notice.set(e),
            }
            busy.set(String::new());
            held.restart();
        });
    });

    let body = match &*held.read() {
        None => rsx! {
            Text { variant: TextVariant::Muted, "Reading subscriptions…" }
        },
        Some(Err(e)) => rsx! {
            div { class: "rounded-xl border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm",
                "Couldn't read subscriptions: {e}"
            }
        },
        Some(Ok(list)) if list.is_empty() => rsx! {
            div { class: "flex flex-col items-center gap-2 rounded-2xl border border-dashed border-border/70 bg-card/40 py-16 text-center",
                Heading { level: HeadingLevel::H3, "Nothing subscribed yet" }
                Text { variant: TextVariant::Muted,
                    "Subscribe to a wiki and its pages resolve inside your own writing — links, search, graph."
                }
            }
        },
        Some(Ok(list)) => {
            let rows = list.clone();
            rsx! {
                div { class: "flex flex-col gap-2",
                    for held_one in rows {
                        {row(held_one, busy(), act)}
                    }
                }
            }
        }
    };

    rsx! {
        div { class: "flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto",
            // Add a source.
            div { class: "flex flex-col gap-2 rounded-xl border border-border/70 bg-card/30 p-3",
                Text { variant: TextVariant::Muted, class: "text-xs",
                    "Subscribe by qualified id — the same thing a reference carries, like "
                    code { class: "rounded bg-muted px-1", "acme.test/music-theory" }
                }
                div { class: "flex flex-wrap items-center gap-2",
                    input {
                        class: "min-w-0 flex-1 rounded-lg border border-border/70 bg-background px-2 py-1 text-sm",
                        placeholder: "domain/slug",
                        value: "{new_ref}",
                        oninput: move |e| new_ref.set(e.value()),
                    }
                    label { class: "flex items-center gap-1.5 text-xs text-muted-foreground",
                        input {
                            r#type: "checkbox",
                            checked: as_resource(),
                            onchange: move |e| as_resource.set(e.checked()),
                        }
                        "Resource (read-only)"
                    }
                    button {
                        class: "rounded-lg border border-border/70 px-3 py-1 text-sm hover:bg-accent",
                        onclick: move |_| {
                            let Some(slug) = org() else { return };
                            let raw = new_ref().trim().to_owned();
                            let Some((domain, source_slug)) = raw.split_once('/') else {
                                notice.set(
                                    "A qualified id is `domain/slug` — `acme.test/music-theory`."
                                        .to_owned(),
                                );
                                return;
                            };
                            let (domain, source_slug) =
                                (domain.to_owned(), source_slug.to_owned());
                            let kind = if as_resource() {
                                wiki_proto::SourceKind::Resource
                            } else {
                                wiki_proto::SourceKind::Wiki
                            };
                            notice.set(String::new());
                            spawn(async move {
                                let title = source_slug.replace('-', " ");
                                match crate::feeds::subscribe_to(
                                    &slug, &domain, &source_slug, &title, kind,
                                )
                                .await
                                {
                                    Ok(()) => {
                                        new_ref.set(String::new());
                                        notice.set(format!(
                                            "Subscribed to {domain}/{source_slug}. Refresh it to bring the copy down."
                                        ));
                                    }
                                    Err(e) => notice.set(e),
                                }
                                held.restart();
                            });
                        },
                        "Subscribe"
                    }
                }
            }

            if !notice().is_empty() {
                div { class: "rounded-lg border border-border/70 bg-muted/40 px-3 py-2 text-xs",
                    "{notice}"
                }
            }

            {body}
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Refresh,
    Unsubscribe { force: bool },
}

fn row(
    held: wiki_proto::HeldSubscription,
    busy: String,
    act: EventHandler<(String, Action)>,
) -> Element {
    let s = held.subscription;
    let qualified = s.qualified();
    let working = busy == qualified;
    let declined = s.declined;
    let editable = s.kind.is_editable();

    let title = if s.title.is_empty() {
        s.slug.clone()
    } else {
        s.title.clone()
    };
    let shell = if declined {
        "flex flex-wrap items-center gap-3 rounded-xl border border-dashed border-border/60 bg-card/20 px-3 py-2 opacity-60"
    } else {
        "flex flex-wrap items-center gap-3 rounded-xl border border-border/70 bg-card/40 px-3 py-2"
    };

    // A subscription and a copy are different facts. Say which.
    let copy_state = if held.files == 0 {
        "no local copy yet".to_owned()
    } else {
        format!("{} file(s) on disk", held.files)
    };

    let q_refresh = qualified.clone();
    let q_drop = qualified.clone();
    let q_force = qualified.clone();

    rsx! {
        div { class: shell,
            div { class: "flex min-w-0 flex-1 flex-col",
                div { class: "flex items-center gap-2",
                    span { class: "truncate font-medium", "{title}" }
                    if s.core {
                        span { class: "rounded bg-muted px-1.5 py-0.5 text-[0.65rem] uppercase tracking-wide text-muted-foreground",
                            "core"
                        }
                    }
                    if !editable {
                        span { class: "rounded bg-muted px-1.5 py-0.5 text-[0.65rem] uppercase tracking-wide text-muted-foreground",
                            "read-only"
                        }
                    }
                    if declined {
                        span { class: "rounded bg-muted px-1.5 py-0.5 text-[0.65rem] uppercase tracking-wide text-muted-foreground",
                            "declined"
                        }
                    }
                }
                span { class: "truncate text-xs text-muted-foreground", "{qualified} — {copy_state}" }
            }
            if !declined {
                button {
                    class: "rounded-lg border border-border/70 px-2 py-1 text-xs hover:bg-accent",
                    disabled: working,
                    onclick: move |_| act.call((q_refresh.clone(), Action::Refresh)),
                    if working { "Working…" } else { "Refresh" }
                }
                button {
                    class: "rounded-lg border border-border/70 px-2 py-1 text-xs hover:bg-accent",
                    disabled: working,
                    onclick: move |_| {
                        act.call((q_drop.clone(), Action::Unsubscribe { force: false }))
                    },
                    "Unsubscribe"
                }
                // Separate control rather than a confirm dialog: the
                // server's refusal carries the count of unpushed work,
                // and a person should read that before this button
                // rather than after a yes/no.
                button {
                    class: "rounded-lg border border-destructive/40 px-2 py-1 text-xs text-destructive hover:bg-destructive/10",
                    disabled: working,
                    onclick: move |_| {
                        act.call((q_force.clone(), Action::Unsubscribe { force: true }))
                    },
                    "Discard copy"
                }
            }
        }
    }
}
