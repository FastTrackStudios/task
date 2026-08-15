//! `/repos` — the forge view.
//!
//! Lists the repos the selected org's forge backend can address
//! (`RepoCatalog::list_repos`) and, under each, that repo's issues
//! (`IssueTracker::list_issues`). It's the read slice over the git
//! feature's two core services — the same surface the CLI's
//! `task issue pull/sync` drives, rendered in the app.
//!
//! When the org's forge is unconfigured (no `TASK_FORGEJO_TOKEN`)
//! the backend returns an auth/forge error; the page tolerates that
//! and shows an empty state rather than blanking.
//!
//! The proto DTOs (`git_proto::Repo` / `issues::Issue`) don't derive
//! `PartialEq`, so child components take the primitive fields they
//! render rather than the whole struct (Dioxus props must be
//! `PartialEq`). `RepoId` itself does derive `PartialEq`/`Hash`, so
//! it's fine to thread through for the per-repo issue fetch.

use architect_ui::prelude::*;
use dioxus::prelude::*;
use git_proto::RepoId;

use crate::orgs::{OrgMeta, OrgSelection};

/// Map a forge issue state onto a status-badge variant.
fn issue_variant(state: git_proto::IssueState) -> StatusBadgeVariant {
    match state {
        git_proto::IssueState::Open => StatusBadgeVariant::Success,
        git_proto::IssueState::Closed => StatusBadgeVariant::Neutral,
    }
}

#[component]
pub fn ReposView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // Every selected org (All mode fans out over all of them, so a
    // repo connected in any org shows up — not just the home org).
    let slugs = use_memo(move || crate::orgs::selected_slugs(&selection.read(), &org_list.read()));

    // Connected repos across the selected orgs, each tagged with its
    // org slug (needed to fetch that repo's issues from the right org).
    let repos = use_resource(move || {
        let slugs = slugs();
        async move {
            let mut out: Vec<(String, RepoId)> = Vec::new();
            for s in slugs {
                // Per-org failures are tolerated — a down/empty org
                // doesn't blank the whole view.
                if let Ok(rids) = crate::feeds::fetch_connected_repos(&s).await {
                    for rid in rids {
                        if !out.iter().any(|(os, orid)| os == &s && orid == &rid) {
                            out.push((s.clone(), rid));
                        }
                    }
                }
            }
            Ok::<Vec<(String, RepoId)>, String>(out)
        }
    });

    let (rows, load_err): (Vec<(String, RepoId)>, Option<String>) = match &*repos.read() {
        Some(Ok(all)) => (all.clone(), None),
        Some(Err(e)) => (Vec::new(), Some(e.clone())),
        None => (Vec::new(), None),
    };

    rsx! {
        div { class: "mx-auto flex max-w-3xl flex-col gap-5 p-4 sm:p-6 lg:p-10",
            div { class: "flex items-center justify-between gap-3",
                Heading { level: HeadingLevel::H1, "Repos" }
                Text { variant: TextVariant::Muted, class: "text-sm", "{rows.len()} repos" }
            }
            Text {
                variant: TextVariant::Muted,
                class: "text-sm -mt-2",
                "Repos connected to a project, with their issues.",
            }

            if let Some(err) = load_err {
                crate::states::InlineError {
                    message: err,
                    label: "Repos".to_string(),
                }
            }

            if rows.is_empty() {
                div { class: "rounded-lg border border-dashed border-border px-4 py-10 text-center",
                    Text {
                        variant: TextVariant::Muted,
                        "No connected repos — bind a project to a repo to populate this view.",
                    }
                }
            } else {
                div { class: "flex flex-col gap-3",
                    for (rslug , rid) in rows {
                        RepoCard {
                            key: "{rslug}:{rid.owner}/{rid.repo}",
                            slug: rslug.clone(),
                            repo_id: rid.clone(),
                            owner: rid.owner.clone(),
                            name: rid.repo.clone(),
                            description: String::new(),
                            private: false,
                            archived: false,
                        }
                    }
                }
            }
        }
    }
}

/// One repo: its slug + description, with its issues listed below.
/// Takes scalar fields (not the `Repo` struct) so the props satisfy
/// Dioxus's `PartialEq` bound; `RepoId` derives it and drives the
/// per-repo issue fetch.
#[component]
fn RepoCard(
    slug: String,
    repo_id: RepoId,
    owner: String,
    name: String,
    description: String,
    private: bool,
    archived: bool,
) -> Element {
    let full_name = format!("{owner}/{name}");

    // Fetch this repo's issues. Keyed on the repo id so each card
    // loads independently. Returns `(number, title, state)` tuples —
    // all `PartialEq`-clean — so the rendered rows don't need the
    // non-`PartialEq` `Issue` struct.
    let issues = use_resource({
        // Capture clones so the `slug` / `repo_id` props stay available
        // for the per-issue `IssueRow` children below.
        let slug = slug.clone();
        let repo_id = repo_id.clone();
        move || {
            let s = slug.clone();
            let rid = repo_id.clone();
            async move {
                if s.is_empty() {
                    Ok(Vec::new())
                } else {
                    crate::feeds::fetch_issues(&s, rid).await.map(|issues| {
                        issues
                            .into_iter()
                            .map(|i| (i.id.0, i.title, i.state))
                            .collect::<Vec<(u64, String, git_proto::IssueState)>>()
                    })
                }
            }
        }
    });

    // ── Live issue changes ────────────────────────────────────
    // `IssueTracker`'s `#[subscribe]` stream. Events name what
    // changed, not the new value (an `Issue` is a forge read), so a
    // hit for this repo re-lists rather than folding — the event is
    // the trigger, the rpc stays the source of truth.
    //
    // Scope: the hub carries what the *server* commits, so this is
    // "someone else's Task session filed/closed an issue here", plus
    // our own writes without a manual refetch. Forge-side changes by
    // other actors still need the poll loop — no forge gives us a
    // push channel we can attach to.
    {
        let sub_slug = slug.clone();
        let filter_repo = repo_id.clone();
        architect::use_stream(
            move |tx| {
                let slug = sub_slug.clone();
                async move {
                    if slug.is_empty() {
                        return false;
                    }
                    let Ok(client) = crate::vox_clients::establish_for::<
                        git_proto::issues::IssueTrackerStreamClient,
                    >(&slug)
                    .await
                    else {
                        return false;
                    };
                    client.issue_events(tx).await.is_ok()
                }
            },
            move |ev: git_proto::GitEvent| {
                let mut issues = issues;
                // The stream is unfiltered across repos; each event
                // names its own.
                let hit = match &ev {
                    git_proto::GitEvent::IssueCreated { repo, .. }
                    | git_proto::GitEvent::IssueUpdated { repo, .. }
                    | git_proto::GitEvent::IssueCommented { repo, .. } => repo == &filter_repo,
                    git_proto::GitEvent::Resync => true,
                    _ => false,
                };
                if hit {
                    issues.restart();
                }
            },
        );
    }

    let (issue_rows, issue_err): (Vec<(u64, String, git_proto::IssueState)>, Option<String>) =
        match &*issues.read() {
            Some(Ok(all)) => (all.clone(), None),
            Some(Err(e)) => (Vec::new(), Some(e.clone())),
            None => (Vec::new(), None),
        };
    let loading = issues.read().is_none();

    rsx! {
        div { class: "flex flex-col gap-2 rounded-xl border border-border bg-card/40 p-3",
            div { class: "flex flex-wrap items-center gap-2",
                Text { class: "text-sm font-medium", "{full_name}" }
                if private {
                    span { class: "rounded bg-muted px-1.5 py-px text-[11px] text-muted-foreground", "private" }
                }
                if archived {
                    span { class: "rounded bg-muted px-1.5 py-px text-[11px] text-muted-foreground", "archived" }
                }
            }
            if !description.is_empty() {
                Text { variant: TextVariant::Muted, class: "text-xs", "{description}" }
            }

            // ── Issues ─────────────────────────────────────────────
            if let Some(err) = issue_err {
                Text { variant: TextVariant::Muted, class: "text-xs", "issues: {err}" }
            } else if loading {
                Text { variant: TextVariant::Muted, class: "text-xs", "Loading issues…" }
            } else if issue_rows.is_empty() {
                Text { variant: TextVariant::Muted, class: "text-xs", "No issues." }
            } else {
                div { class: "mt-1 flex flex-col gap-1",
                    for (number , title , state) in issue_rows {
                        IssueRow {
                            key: "{number}",
                            slug: slug.clone(),
                            repo_id: repo_id.clone(),
                            number,
                            title,
                            state,
                        }
                    }
                }
            }
        }
    }
}

/// One issue row + its forge comments (the conversation), loaded lazily
/// from `IssueTracker::list_comments`. Comments map to `(author, body)`
/// tuples so the rendered rows stay `PartialEq`-clean.
#[component]
fn IssueRow(
    slug: String,
    repo_id: RepoId,
    number: u64,
    title: String,
    state: git_proto::IssueState,
) -> Element {
    let comments = use_resource(move || {
        let s = slug.clone();
        let rid = repo_id.clone();
        async move {
            if s.is_empty() {
                Ok(Vec::new())
            } else {
                crate::feeds::fetch_issue_comments(&s, rid, number)
                    .await
                    .map(|cs| {
                        cs.into_iter()
                            .map(|c| (c.author.login, c.body))
                            .collect::<Vec<(String, String)>>()
                    })
            }
        }
    });
    let comment_rows: Vec<(String, String)> = match &*comments.read() {
        Some(Ok(c)) => c.clone(),
        _ => Vec::new(),
    };

    rsx! {
        div { class: "flex flex-col gap-1 rounded-lg border border-border bg-background/40 px-2 py-1",
            div { class: "flex items-center gap-2",
                Text { variant: TextVariant::Muted, class: "shrink-0 text-[11px] font-mono", "#{number}" }
                Text { class: "min-w-0 flex-1 truncate text-xs", "{title}" }
                StatusBadge {
                    variant: issue_variant(state),
                    label: if state == git_proto::IssueState::Open { "open".to_string() } else { "closed".to_string() },
                }
            }
            if !comment_rows.is_empty() {
                div { class: "ml-3 flex flex-col gap-1 border-l border-border pl-3",
                    for (i , (author , body)) in comment_rows.into_iter().enumerate() {
                        div { key: "{i}", class: "flex flex-col",
                            Text { variant: TextVariant::Muted, class: "text-[11px] font-medium", "{author}" }
                            Text { class: "text-xs", "{body}" }
                        }
                    }
                }
            }
        }
    }
}
