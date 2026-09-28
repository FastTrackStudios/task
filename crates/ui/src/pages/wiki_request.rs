//! `/wiki/w/:org/:wiki/request/:id` — one Edit Request, as its reviewer
//! and its proposer see it.
//!
//! t[impl wiki.edit.reviewable] — the request's pages as a diff against
//! the wiki now, whether each still applies (a stale request that merges
//! cleanly shows what would land), who proposed it and why, and — for an
//! Editor — Claim, Accept, Send back and Reject. The proposer sees the
//! same page and where it stands; a returned request says what the
//! reviewer asked for.

use architect_ui::prelude::*;
use dioxus::prelude::*;
use wiki_proto::service::edits::{EditRequest, EditStatus, EditsClient, PageDiff};

use crate::pages::wiki_access::{name_of, use_member_names, use_wiki_access};
use crate::routes::Route;

#[component]
pub fn WikiRequestView(org: String, wiki: String, id: String) -> Element {
    let org_sig = {
        let org = org.clone();
        use_memo(use_reactive!(|org| org))
    };
    let access = use_wiki_access(org_sig, wiki.clone());
    let names = use_member_names(org_sig);
    let account = use_context::<Signal<Option<crate::auth::ActiveAccount>>>();
    let me = account.read().as_ref().map(|a| a.user_id.to_string());
    let mut reload = use_signal(|| 0u32);
    let mut note = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut action_error = use_signal(|| None::<String>);

    let parsed = uuid::Uuid::parse_str(&id).ok();
    let wiki_for_load = wiki.clone();
    let loaded = use_resource(move || {
        let _ = reload();
        let slug = org_sig();
        let wiki = wiki_for_load.clone();
        async move {
            let id = parsed.ok_or_else(|| "not a request id".to_owned())?;
            let client = crate::vox_clients::establish_for::<EditsClient>(&slug).await?;
            let request = client
                .get_edit_request(wiki.clone(), id)
                .await
                .map_err(|e| format!("{e:?}"))?;
            let diffs = client
                .diff_edit_request(wiki, id)
                .await
                .map_err(|e| format!("{e:?}"))?;
            Ok::<_, String>((request, diffs))
        }
    });

    // One action on the request, then re-read it.
    let act = use_callback({
        let wiki = wiki.clone();
        move |verb: Verb| {
            let Some(id) = parsed else { return };
            let slug = org_sig.peek().clone();
            let wiki = wiki.clone();
            let reason = note.peek().trim().to_owned();
            if matches!(verb, Verb::Reject | Verb::Return) && reason.is_empty() {
                action_error.set(Some("Say why — the proposer reads it.".into()));
                return;
            }
            busy.set(true);
            action_error.set(None);
            spawn(async move {
                let result = async {
                    let c = crate::vox_clients::establish_for::<EditsClient>(&slug).await?;
                    let r = match verb {
                        Verb::Claim => c.claim_edit_request(wiki, id).await,
                        Verb::Release => c.release_edit_request(wiki, id).await,
                        Verb::Accept => c.accept_edit_request(wiki, id).await,
                        Verb::Reject => c.reject_edit_request(wiki, id, reason).await,
                        Verb::Return => c.return_edit_request(wiki, id, reason).await,
                    };
                    r.map_err(|e| format!("{e:?}"))
                }
                .await;
                busy.set(false);
                match result {
                    Ok(_) => {
                        note.set(String::new());
                        reload += 1;
                    }
                    Err(e) => action_error.set(Some(e)),
                }
            });
        }
    });

    let back = rsx! {
        Link {
            to: Route::WikiHomeRoute { org: org.clone(), wiki: wiki.clone() },
            class: "text-xs text-muted-foreground hover:text-foreground",
            "← {wiki}"
        }
    };

    let body = match &*loaded.read() {
        None => rsx! { Text { variant: TextVariant::Muted, "Loading the request…" } },
        Some(Err(e)) => rsx! {
            crate::states::ErrorState { title: "Couldn't open this request", message: e.clone() }
        },
        Some(Ok((request, diffs))) => {
            let names = names.read();
            let is_editor = access.read().as_ref().is_some_and(|a| a.is_editor);
            let mine = me.as_deref() == Some(request.proposer.as_str());
            let claimed_by_me = me.as_deref() == Some(request.claimed_by.as_str());
            let open = request.status.is_open();
            let all_apply = diffs.iter().all(|d| d.applies);
            rsx! {
                {header(request, &names)}
                if request.status == EditStatus::Returned && !request.resolution.is_empty() {
                    div { class: "rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm",
                        "Sent back: {request.resolution}"
                    }
                }
                if matches!(request.status, EditStatus::Rejected | EditStatus::Closed) && !request.resolution.is_empty() {
                    div { class: "rounded-lg border border-border/70 bg-card/40 px-3 py-2 text-sm text-muted-foreground",
                        "{request.status.as_str()}: {request.resolution}"
                    }
                }
                if open {
                    for d in diffs.iter() {
                        {page_diff(d)}
                    }
                } else {
                    // Resolved: the wiki now may already hold the change,
                    // so show what the request changed — its base to its
                    // proposal — not a diff against today.
                    for c in request.changes.iter() {
                        {change_diff(c, request.status)}
                    }
                }
                if open && is_editor && !mine {
                    div { class: "flex flex-col gap-2 rounded-xl border border-border/70 bg-card/40 p-3",
                        "data-testid": "review-actions",
                        textarea {
                            class: "min-h-16 rounded-md border border-border/70 bg-background px-2 py-1 text-sm",
                            placeholder: "A note to the proposer (needed to send back or reject)",
                            value: "{note}",
                            oninput: move |e| note.set(e.value()),
                        }
                        div { class: "flex flex-wrap items-center justify-end gap-2",
                            if claimed_by_me {
                                Button { variant: ButtonVariant::Ghost, size: ButtonSize::Small, disabled: busy(),
                                    on_click: move |_| act.call(Verb::Release),
                                    "Release"
                                }
                            } else if request.claimed_by.is_empty() {
                                Button { variant: ButtonVariant::Ghost, size: ButtonSize::Small, disabled: busy(),
                                    on_click: move |_| act.call(Verb::Claim),
                                    "Claim"
                                }
                            }
                            Button { variant: ButtonVariant::Outline, size: ButtonSize::Small, disabled: busy(),
                                on_click: move |_| act.call(Verb::Return),
                                "Send back"
                            }
                            Button { variant: ButtonVariant::Destructive, size: ButtonSize::Small, disabled: busy(),
                                on_click: move |_| act.call(Verb::Reject),
                                "Reject"
                            }
                            Button { variant: ButtonVariant::Primary, size: ButtonSize::Small,
                                disabled: busy() || !all_apply,
                                on_click: move |_| act.call(Verb::Accept),
                                if all_apply { "Accept" } else { "Conflicts — can't accept" }
                            }
                        }
                    }
                } else if open && mine {
                    Text { variant: TextVariant::Muted,
                        "Waiting for an Editor to review it."
                    }
                }
                if let Some(e) = action_error() {
                    span { class: "text-sm text-destructive", "{e}" }
                }
            }
        }
    };

    rsx! {
        div { class: "mx-auto flex h-full w-full max-w-4xl flex-col gap-4 overflow-y-auto p-4 sm:p-6 lg:p-8",
            {back}
            {body}
        }
    }
}

#[derive(Clone, Copy)]
enum Verb {
    Claim,
    Release,
    Accept,
    Reject,
    Return,
}

fn header(request: &EditRequest, names: &std::collections::HashMap<String, String>) -> Element {
    let who = name_of(names, &request.proposer);
    let opened = request
        .opened_at
        .get(..10)
        .unwrap_or(&request.opened_at)
        .to_owned();
    let claim = (!request.claimed_by.is_empty()).then(|| name_of(names, &request.claimed_by));
    rsx! {
        header { class: "flex flex-col gap-1",
            div { class: "flex flex-wrap items-baseline gap-3",
                Heading { level: HeadingLevel::H1, class: "tracking-tight", "{request.title}" }
                {status_pill(request.status)}
                if request.held {
                    span { class: "rounded-full border border-amber-500/40 px-2 py-0.5 text-xs text-amber-600 dark:text-amber-400",
                        title: "The proposer is not someone this wiki vouches for; it is not published on their behalf.",
                        "held"
                    }
                }
            }
            Text { variant: TextVariant::Muted, "Proposed by {who} on {opened}" }
            if let Some(c) = claim {
                Text { variant: TextVariant::Muted, "{c} is reviewing it." }
            }
            if !request.summary.is_empty() {
                p { class: "mt-1 whitespace-pre-wrap text-sm", "{request.summary}" }
            }
        }
    }
}

/// A request's status as a small coloured pill.
#[must_use]
pub fn status_pill(status: EditStatus) -> Element {
    let class = match status {
        EditStatus::Open => "border-sky-500/40 text-sky-600 dark:text-sky-400",
        EditStatus::Returned => "border-amber-500/40 text-amber-600 dark:text-amber-400",
        EditStatus::Accepted | EditStatus::Landing => {
            "border-emerald-500/40 text-emerald-600 dark:text-emerald-400"
        }
        EditStatus::Rejected | EditStatus::Closed => "border-border text-muted-foreground",
    };
    rsx! {
        span { class: "rounded-full border px-2 py-0.5 text-xs font-medium {class}", "{status.as_str()}" }
    }
}

/// One page of the request: a unified line diff of the wiki now against
/// what would land.
fn page_diff(d: &PageDiff) -> Element {
    // What would land when it applies (the three-way merge of a stale
    // request), else what was proposed.
    let after = if d.applies && !d.merged.is_empty() {
        d.merged.as_str()
    } else {
        d.proposed.as_str()
    };
    let lines = diff_lines(&d.current, after);
    let state = if d.current.is_empty() {
        "new page"
    } else if d.proposed.is_empty() {
        "delete"
    } else if !d.applies {
        "conflicts with a later change"
    } else if d.stale {
        "merges over a later change"
    } else {
        "applies"
    };
    rsx! {
        section { class: "overflow-hidden rounded-xl border border-border/70",
            div { class: "flex items-center justify-between gap-2 border-b border-border/70 bg-card/60 px-3 py-1.5 text-xs",
                span { class: "font-mono", "{d.path}" }
                span { class: if d.applies { "text-muted-foreground" } else { "text-destructive" }, "{state}" }
            }
            {diff_body(lines)}
        }
    }
}

/// One page of a resolved request: what it proposed, against the page
/// the proposer saw.
fn change_diff(c: &wiki_proto::service::edits::PageChange, status: EditStatus) -> Element {
    let after = if c.delete { "" } else { c.markdown.as_str() };
    let lines = diff_lines(&c.base_markdown, after);
    let label = match status {
        EditStatus::Accepted => "landed",
        EditStatus::Landing => "landing",
        _ => "not applied",
    };
    rsx! {
        section { class: "overflow-hidden rounded-xl border border-border/70",
            div { class: "flex items-center justify-between gap-2 border-b border-border/70 bg-card/60 px-3 py-1.5 text-xs",
                span { class: "font-mono", "{c.path}" }
                span { class: "text-muted-foreground", "{label}" }
            }
            {diff_body(lines)}
        }
    }
}

fn diff_body(lines: Vec<(LineKind, String)>) -> Element {
    rsx! {
        pre { class: "overflow-x-auto py-1 font-mono text-xs leading-5",
            for (i, (kind, text)) in lines.into_iter().enumerate() {
                div {
                    key: "{i}",
                    class: match kind {
                        LineKind::Added => "bg-emerald-500/10 px-3 text-emerald-700 dark:text-emerald-300",
                        LineKind::Removed => "bg-destructive/10 px-3 text-destructive",
                        LineKind::Context => "px-3 text-muted-foreground",
                        LineKind::Gap => "px-3 text-muted-foreground/60",
                    },
                    "{text}"
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LineKind {
    Added,
    Removed,
    Context,
    Gap,
}

/// The unified diff of `before` → `after` as display lines, three lines
/// of context around each change.
fn diff_lines(before: &str, after: &str) -> Vec<(LineKind, String)> {
    let patch = diffy::create_patch(before, after);
    let mut out = Vec::new();
    for (i, hunk) in patch.hunks().iter().enumerate() {
        if i > 0 {
            out.push((LineKind::Gap, "⋯".to_owned()));
        }
        for line in hunk.lines() {
            let (kind, text) = match line {
                diffy::Line::Insert(t) => (LineKind::Added, format!("+ {t}")),
                diffy::Line::Delete(t) => (LineKind::Removed, format!("- {t}")),
                diffy::Line::Context(t) => (LineKind::Context, format!("  {t}")),
            };
            out.push((kind, text.trim_end_matches('\n').to_owned()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_line_is_a_removal_and_an_addition_in_context() {
        let before = "a\nb\nc\n";
        let after = "a\nB\nc\n";
        let lines = diff_lines(before, after);
        assert_eq!(
            lines,
            vec![
                (LineKind::Context, "  a".into()),
                (LineKind::Removed, "- b".into()),
                (LineKind::Added, "+ B".into()),
                (LineKind::Context, "  c".into()),
            ]
        );
    }

    #[test]
    fn identical_pages_have_no_lines() {
        assert!(diff_lines("same\n", "same\n").is_empty());
    }
}
