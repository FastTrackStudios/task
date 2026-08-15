//! Forge management views — issues + pull requests of a connected repo,
//! each with its conversation, viewable + manageable (post comments,
//! open/close an issue, merge a PR). Used in the project detail page.
//!
//! Vox-aware (each row owns its `use_resource` fetches, like the
//! `/repos` `RepoCard`). Writes go through the server's configured forge
//! identity (codywright); a shared `refresh` signal re-fetches after any
//! write. Comments map to `(author, body)` tuples so rows stay
//! `PartialEq`-clean (the proto DTOs aren't).

use dioxus::prelude::*;
use architect_ui::prelude::*;
use git_proto::{IssueState, MergeMethod, PullRequestState, RepoId};

/// Map a forge issue state onto a status-badge variant.
#[must_use]
pub fn issue_variant(state: IssueState) -> StatusBadgeVariant {
    match state {
        IssueState::Open => StatusBadgeVariant::Success,
        IssueState::Closed => StatusBadgeVariant::Neutral,
    }
}

fn pr_variant(state: PullRequestState) -> StatusBadgeVariant {
    match state {
        PullRequestState::Open => StatusBadgeVariant::Success,
        PullRequestState::Merged => StatusBadgeVariant::Neutral,
        PullRequestState::Closed => StatusBadgeVariant::Danger,
    }
}

fn pr_label(state: PullRequestState) -> String {
    match state {
        PullRequestState::Open => "open",
        PullRequestState::Merged => "merged",
        PullRequestState::Closed => "closed",
    }
    .to_string()
}

/// The connected repo's issues + pull requests with their conversations.
#[component]
pub fn ForgePanel(slug: String, repo_id: RepoId) -> Element {
    // One refresh tick re-fetches everything (lists + per-row comments)
    // after any write.
    let refresh = use_signal(|| 0u32);

    let issues = use_resource({
        let slug = slug.clone();
        let repo_id = repo_id.clone();
        move || {
            let _ = refresh.read();
            let s = slug.clone();
            let r = repo_id.clone();
            async move {
                crate::feeds::fetch_issues(&s, r).await.map(|v| {
                    v.into_iter()
                        .map(|i| (i.id.0, i.title, i.state))
                        .collect::<Vec<(u64, String, IssueState)>>()
                })
            }
        }
    });
    let prs = use_resource({
        let slug = slug.clone();
        let repo_id = repo_id.clone();
        move || {
            let _ = refresh.read();
            let s = slug.clone();
            let r = repo_id.clone();
            async move {
                crate::feeds::fetch_pull_requests(&s, r).await.map(|v| {
                    v.into_iter()
                        .map(|p| (p.id.0, p.title, p.state, p.base, p.head, p.draft))
                        .collect::<Vec<(u64, String, PullRequestState, String, String, bool)>>()
                })
            }
        }
    });

    let issue_rows = match &*issues.read() {
        Some(Ok(v)) => v.clone(),
        _ => Vec::new(),
    };
    let pr_rows = match &*prs.read() {
        Some(Ok(v)) => v.clone(),
        _ => Vec::new(),
    };

    rsx! {
        div { class: "flex flex-col gap-4",
            // ── Issues ──────────────────────────────────────────────
            div { class: "flex flex-col gap-2",
                Heading { level: HeadingLevel::H3, "Issues" }
                if issue_rows.is_empty() {
                    Text { variant: TextVariant::Muted, class: "text-sm", "No issues." }
                }
                for (number , title , state) in issue_rows {
                    IssueManageRow {
                        key: "i{number}",
                        slug: slug.clone(),
                        repo_id: repo_id.clone(),
                        number,
                        title,
                        state,
                        refresh,
                    }
                }
            }
            // ── Pull requests ───────────────────────────────────────
            div { class: "flex flex-col gap-2",
                Heading { level: HeadingLevel::H3, "Pull requests" }
                if pr_rows.is_empty() {
                    Text { variant: TextVariant::Muted, class: "text-sm", "No pull requests." }
                }
                for (number , title , state , base , head , draft) in pr_rows {
                    PrManageRow {
                        key: "p{number}",
                        slug: slug.clone(),
                        repo_id: repo_id.clone(),
                        number,
                        title,
                        state,
                        base,
                        head,
                        draft,
                        refresh,
                    }
                }
            }
        }
    }
}

/// A comment list + composer. `comments` in, `on_post` out.
#[component]
fn ConversationView(comments: Vec<(String, String)>, on_post: EventHandler<String>) -> Element {
    let mut draft = use_signal(String::new);
    rsx! {
        div { class: "ml-3 flex flex-col gap-1 border-l border-border pl-3",
            for (i , (author , body)) in comments.into_iter().enumerate() {
                div { key: "{i}", class: "flex flex-col",
                    Text { variant: TextVariant::Muted, class: "text-[11px] font-medium", "{author}" }
                    Text { class: "text-xs", "{body}" }
                }
            }
            div { class: "mt-1 flex items-center gap-2",
                Input { value: draft, placeholder: "Comment…", on_change: move |_| {} }
                Button {
                    variant: ButtonVariant::Secondary,
                    size: ButtonSize::Small,
                    on_click: move |_| {
                        let b = draft.read().trim().to_string();
                        if !b.is_empty() {
                            on_post.call(b);
                            draft.set(String::new());
                        }
                    },
                    "Comment"
                }
            }
        }
    }
}

#[component]
fn IssueManageRow(
    slug: String,
    repo_id: RepoId,
    number: u64,
    title: String,
    state: IssueState,
    refresh: Signal<u32>,
) -> Element {
    // Conversation is collapsed by default — the comments fetch (a live
    // forge round-trip) only fires once the row is expanded, so opening a
    // project no longer N+1s the forge for every issue's thread.
    let mut expanded = use_signal(|| false);
    let comments = use_resource({
        let slug = slug.clone();
        let repo_id = repo_id.clone();
        move || {
            let _ = refresh.read();
            let open = expanded();
            let s = slug.clone();
            let r = repo_id.clone();
            async move {
                if !open {
                    return Ok::<Vec<(String, String)>, String>(Vec::new());
                }
                crate::feeds::fetch_issue_comments(&s, r, number)
                    .await
                    .map(|cs| {
                        cs.into_iter()
                            .map(|c| (c.author.login, c.body))
                            .collect::<Vec<(String, String)>>()
                    })
            }
        }
    });
    let comment_rows = match &*comments.read() {
        Some(Ok(c)) => c.clone(),
        _ => Vec::new(),
    };
    let is_open = state == IssueState::Open;

    // Per-handler clones so each `move` closure owns its own copy.
    let toggle_slug = slug.clone();
    let toggle_repo = repo_id.clone();
    let post_slug = slug.clone();
    let post_repo = repo_id.clone();
    let mut refresh = refresh;

    rsx! {
        div { class: "flex flex-col gap-1 rounded-lg border border-border bg-background/40 px-2 py-1",
            div { class: "flex items-center gap-2",
                Text { variant: TextVariant::Muted, class: "shrink-0 text-[11px] font-mono", "#{number}" }
                Text { class: "min-w-0 flex-1 truncate text-xs", "{title}" }
                StatusBadge {
                    variant: issue_variant(state),
                    label: if is_open { "open".to_string() } else { "closed".to_string() },
                }
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    on_click: move |_| expanded.toggle(),
                    if expanded() { "Hide" } else { "Comments" }
                }
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    on_click: move |_| {
                        let s = toggle_slug.clone();
                        let r = toggle_repo.clone();
                        let next = if is_open { IssueState::Closed } else { IssueState::Open };
                        spawn(async move {
                            if let Err(e) = crate::feeds::set_issue_state(&s, r, number, next).await {
                                tracing::warn!("set issue state: {e}");
                            }
                            refresh.with_mut(|x| *x += 1);
                        });
                    },
                    if is_open { "Close" } else { "Reopen" }
                }
            }
            if expanded() {
                ConversationView {
                    comments: comment_rows,
                    on_post: move |body: String| {
                        let s = post_slug.clone();
                        let r = post_repo.clone();
                        spawn(async move {
                            if let Err(e) = crate::feeds::post_issue_comment(&s, r, number, body).await {
                                tracing::warn!("post comment: {e}");
                            }
                            refresh.with_mut(|x| *x += 1);
                        });
                    },
                }
            }
        }
    }
}

#[component]
#[allow(clippy::too_many_arguments)]
fn PrManageRow(
    slug: String,
    repo_id: RepoId,
    number: u64,
    title: String,
    state: PullRequestState,
    base: String,
    head: String,
    draft: bool,
    refresh: Signal<u32>,
) -> Element {
    // Conversation collapsed by default; comments fetch lazily on expand.
    // PRs share the issue index, so comments come from the issue endpoint.
    let mut expanded = use_signal(|| false);
    let comments = use_resource({
        let slug = slug.clone();
        let repo_id = repo_id.clone();
        move || {
            let _ = refresh.read();
            let open = expanded();
            let s = slug.clone();
            let r = repo_id.clone();
            async move {
                if !open {
                    return Ok::<Vec<(String, String)>, String>(Vec::new());
                }
                crate::feeds::fetch_issue_comments(&s, r, number)
                    .await
                    .map(|cs| {
                        cs.into_iter()
                            .map(|c| (c.author.login, c.body))
                            .collect::<Vec<(String, String)>>()
                    })
            }
        }
    });
    let comment_rows = match &*comments.read() {
        Some(Ok(c)) => c.clone(),
        _ => Vec::new(),
    };
    let mergeable = state == PullRequestState::Open && !draft;

    let merge_slug = slug.clone();
    let merge_repo = repo_id.clone();
    let post_slug = slug.clone();
    let post_repo = repo_id.clone();
    let mut refresh = refresh;

    rsx! {
        div { class: "flex flex-col gap-1 rounded-lg border border-border bg-background/40 px-2 py-1",
            div { class: "flex items-center gap-2",
                Text { variant: TextVariant::Muted, class: "shrink-0 text-[11px] font-mono", "#{number}" }
                Text { class: "min-w-0 flex-1 truncate text-xs", "{title}" }
                if draft {
                    span { class: "rounded bg-muted px-1.5 py-px text-[11px] text-muted-foreground", "draft" }
                }
                StatusBadge { variant: pr_variant(state), label: pr_label(state) }
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    on_click: move |_| expanded.toggle(),
                    if expanded() { "Hide" } else { "Comments" }
                }
                if mergeable {
                    Button {
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Small,
                        on_click: move |_| {
                            let s = merge_slug.clone();
                            let r = merge_repo.clone();
                            spawn(async move {
                                if let Err(e) = crate::feeds::merge_pull_request(&s, r, number, MergeMethod::Merge).await {
                                    tracing::warn!("merge PR: {e}");
                                }
                                refresh.with_mut(|x| *x += 1);
                            });
                        },
                        "Merge"
                    }
                }
            }
            Text { variant: TextVariant::Muted, class: "text-[11px] font-mono", "{head} → {base}" }
            if expanded() {
                ConversationView {
                    comments: comment_rows,
                    on_post: move |body: String| {
                        let s = post_slug.clone();
                        let r = post_repo.clone();
                        spawn(async move {
                            if let Err(e) = crate::feeds::post_issue_comment(&s, r, number, body).await {
                                tracing::warn!("post comment: {e}");
                            }
                            refresh.with_mut(|x| *x += 1);
                        });
                    },
                }
            }
        }
    }
}
