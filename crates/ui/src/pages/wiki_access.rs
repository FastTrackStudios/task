//! Who may change a wiki, as the page in front of them needs to know it —
//! and the way in for everyone else.
//!
//! A wiki with no Editors is one the org's members write directly. Once
//! Editors are declared (`wiki.edit.editor`), only they do; everyone else
//! proposes a change as an **Edit Request** that an Editor reviews. The
//! server enforces that on every write path. This module is what makes
//! the page *say* so rather than let a person type into a page whose save
//! will be refused: [`use_wiki_access`] reads the Editors, and
//! [`ProposeBar`] turns the open page into a draft and sends it as a
//! request.

use architect_ui::prelude::*;
use dioxus::prelude::*;
use wiki_proto::service::edits::{NewEditRequest, PageChange};

use crate::document_session::DocumentSession;
use crate::pages::note_view::WriteMode;

/// What the signed-in person may do with one wiki.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct WikiAccess {
    /// The wiki's title, for a sentence about it (its slug when untitled).
    pub title: String,
    /// The accounts holding Editor. Empty: the org's members write it.
    pub editors: Vec<String>,
    /// Whether the signed-in person writes directly.
    pub can_edit: bool,
    /// Whether they are one of the Editors (and so reviews requests).
    pub is_editor: bool,
}

impl WikiAccess {
    /// A wiki governed by its Editors.
    #[must_use]
    pub fn has_edit_lane(&self) -> bool {
        !self.editors.is_empty()
    }
}

/// The signed-in person's access to `wiki` of `org`. `None` while it is
/// being read; a read that fails is treated as "writes directly", which
/// is what it was before this was asked — the server still refuses a
/// write it would not take.
pub fn use_wiki_access(org: Memo<String>, wiki: String) -> Memo<Option<WikiAccess>> {
    let account = use_context::<Signal<Option<crate::auth::ActiveAccount>>>();
    let wiki_sig = use_signal(|| wiki.clone());
    let described = use_resource(move || {
        let slug = org();
        let wiki = wiki_sig();
        async move {
            let client = crate::vox_clients::establish_for::<
                wiki_proto::service::registry::RegistryClient,
            >(&slug)
            .await?;
            client
                .describe_wiki(wiki)
                .await
                .map_err(|e| format!("{e:?}"))
        }
    });
    use_memo(move || {
        let me = account.read().as_ref().map(|a| a.user_id.to_string());
        let slug = wiki_sig.read().clone();
        match &*described.read() {
            None => None,
            Some(Err(e)) => {
                tracing::debug!("wiki access: {slug} undescribed ({e}); writing directly");
                Some(WikiAccess {
                    title: slug,
                    can_edit: true,
                    ..WikiAccess::default()
                })
            }
            Some(Ok(d)) => {
                let editors = d.config.editors.clone();
                let is_editor = me.as_ref().is_some_and(|m| editors.contains(m));
                Some(WikiAccess {
                    title: if d.summary.title.is_empty() {
                        slug
                    } else {
                        d.summary.title.clone()
                    },
                    can_edit: editors.is_empty() || is_editor,
                    is_editor,
                    editors,
                })
            }
        }
    })
}

/// Account id → display name for the org's members, so a request reads
/// "Sam" rather than a uuid. Empty until it loads, and on failure.
pub fn use_member_names(org: Memo<String>) -> Memo<std::collections::HashMap<String, String>> {
    let account = use_context::<Signal<Option<crate::auth::ActiveAccount>>>();
    let members = use_resource(move || {
        let slug = org();
        let token = account.read().as_ref().map(|a| a.token.clone());
        async move { crate::feeds::fetch_org_members(&slug, token.unwrap_or_default()).await }
    });
    use_memo(move || match &*members.read() {
        Some(Ok(list)) => list
            .iter()
            .map(|m| {
                let name = if m.name.is_empty() {
                    m.email.clone()
                } else {
                    m.name.clone()
                };
                (m.user_id.to_string(), name)
            })
            .collect(),
        _ => std::collections::HashMap::new(),
    })
}

/// A person, by name when the org knows them.
#[must_use]
pub fn name_of(names: &std::collections::HashMap<String, String>, id: &str) -> String {
    names.get(id).cloned().unwrap_or_else(|| {
        let short: String = id.chars().take(8).collect();
        format!("account {short}")
    })
}

/// How the open page may be changed, given access and whether the person
/// has chosen to propose an edit.
#[must_use]
pub fn write_mode(access: Option<&WikiAccess>, proposing: bool) -> WriteMode {
    match access {
        // Until it is known, nothing can be typed that might not save.
        None => WriteMode::ReadOnly,
        Some(a) if a.can_edit => WriteMode::Direct,
        Some(_) if proposing => WriteMode::Draft,
        Some(_) => WriteMode::ReadOnly,
    }
}

/// The strip above a page this person may not write: an explanation and
/// "Propose an edit", then — once proposing — a title, a note to the
/// reviewer, and Send.
#[component]
pub fn ProposeBar(
    org: Memo<String>,
    wiki: String,
    access: WikiAccess,
    proposing: Signal<bool>,
    session: Signal<Option<DocumentSession>>,
) -> Element {
    let mut proposing = proposing;
    let mut title = use_signal(String::new);
    let mut summary = use_signal(String::new);
    let mut sending = use_signal(|| false);
    let mut outcome = use_signal(|| None::<Result<String, String>>);
    let editors = access.editors.len();
    let reviewers = if editors == 1 {
        "its Editor".to_owned()
    } else {
        format!("its {editors} Editors")
    };

    let send = {
        let wiki = wiki.clone();
        move |_| {
            let Some(s) = *session.peek() else { return };
            let Some(draft) = s.draft() else { return };
            if !draft.changed() {
                outcome.set(Some(Err(
                    "Nothing to send yet — change the page first.".into()
                )));
                return;
            }
            let heading = title.peek().trim().to_owned();
            let heading = if heading.is_empty() {
                format!("Change {}", crate::pages::vault::basename_of(&draft.path))
            } else {
                heading
            };
            let request = NewEditRequest {
                title: heading,
                summary: summary.peek().trim().to_owned(),
                changes: vec![PageChange {
                    path: draft.path.clone(),
                    base_sha256: draft.base_sha256.clone(),
                    base_markdown: draft.base_text.clone(),
                    markdown: draft.text.clone(),
                    delete: false,
                }],
                request_review: false,
            };
            let slug = org.peek().clone();
            let wiki = wiki.clone();
            sending.set(true);
            spawn(async move {
                let result = async {
                    let client = crate::vox_clients::establish_for::<
                        wiki_proto::service::edits::EditsClient,
                    >(&slug)
                    .await?;
                    client
                        .open_edit_request(wiki, request)
                        .await
                        .map_err(|e| format!("{e:?}"))
                }
                .await;
                sending.set(false);
                match result {
                    Ok(req) => {
                        s.discard_draft();
                        title.set(String::new());
                        summary.set(String::new());
                        proposing.set(false);
                        outcome.set(Some(Ok(format!(
                            "Sent “{}” for review. You'll see it on the wiki's Edit Requests.",
                            req.title
                        ))));
                    }
                    Err(e) => outcome.set(Some(Err(format!("Couldn't send it: {e}")))),
                }
            });
        }
    };

    rsx! {
        div { class: "mx-auto mb-2 flex w-full max-w-3xl flex-col gap-2 rounded-xl border border-border/70 bg-card/40 px-4 py-3 text-sm",
            "data-testid": "propose-bar",
            if !proposing() {
                div { class: "flex flex-wrap items-center justify-between gap-3",
                    span { class: "text-muted-foreground",
                        "{access.title} is edited by {reviewers}. You can read this page and propose a change."
                    }
                    Button {
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Small,
                        on_click: move |_| {
                            outcome.set(None);
                            proposing.set(true);
                        },
                        "Propose an edit"
                    }
                }
            } else {
                div { class: "flex flex-col gap-2",
                    span { class: "text-muted-foreground",
                        "Edit the page below, then send it to {reviewers}. Nothing changes in the wiki until it is accepted."
                    }
                    input {
                        class: "rounded-md border border-border/70 bg-background px-2 py-1",
                        placeholder: "What does this change? (a short title)",
                        value: "{title}",
                        oninput: move |e| title.set(e.value()),
                    }
                    textarea {
                        class: "min-h-16 rounded-md border border-border/70 bg-background px-2 py-1",
                        placeholder: "Anything the reviewer should know (optional)",
                        value: "{summary}",
                        oninput: move |e| summary.set(e.value()),
                    }
                    div { class: "flex items-center justify-end gap-2",
                        Button {
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Small,
                            on_click: move |_| {
                                if let Some(s) = *session.peek() {
                                    s.discard_draft();
                                }
                                proposing.set(false);
                            },
                            "Discard"
                        }
                        Button {
                            variant: ButtonVariant::Primary,
                            size: ButtonSize::Small,
                            disabled: sending(),
                            on_click: send,
                            if sending() { "Sending…" } else { "Send for review" }
                        }
                    }
                }
            }
            match outcome() {
                Some(Ok(msg)) => rsx! { span { class: "text-emerald-600 dark:text-emerald-400", "{msg}" } },
                Some(Err(msg)) => rsx! { span { class: "text-destructive", "{msg}" } },
                None => rsx! {},
            }
        }
    }
}
