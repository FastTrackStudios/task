//! The comment rail: the scrolling thread + the composer pinned at
//! the bottom.
//!
//! Every comment is frame feedback — clicking a row lands on its
//! frame, holds it, and reveals its drawing. The composer shows
//! exactly which frame a comment will pin to (the amber timecode
//! chip, detachable), and a pending drawing forces the pin: drawn
//! feedback without its frame would be a lie.

use architect_ui::lucide_dioxus::{Clock, Pencil, SendHorizontal, X};
use architect_ui::prelude::*;
use dioxus::prelude::*;
use files_proto::{NewReviewComment, ReviewComment};

use super::{
    DrawCtx, PlayerCtx, ReviewData, UNPINNED, avatar_css, display_author, display_timecode,
    ensure_review, initials, is_pinned, pause, post_comment, remove_comment, seek_to,
};

#[component]
pub(crate) fn CommentsRail(
    video_id: String,
    /// The version the player is showing — what a fresh comment
    /// records (a reviewer pinned to v2 is commenting on v2, AC 2).
    watching: ReadSignal<Option<String>>,
    /// The chain head — what an older comment's badge contrasts with.
    head: ReadSignal<Option<String>>,
) -> Element {
    let data = use_context::<ReviewData>();
    let head_hex = use_memo(move || head().unwrap_or_default());

    rsx! {
        div { class: "flex h-full min-h-0 flex-col",
            // ── thread ──────────────────────────────────────────
            div { class: "flex-1 min-h-0 overflow-y-auto px-3 py-2 flex flex-col gap-1.5",
                {match &*data.scope.read_unchecked() {
                    None => rsx! {
                        Text { variant: TextVariant::Muted, class: "text-xs", "Checking for a review…" }
                    },
                    Some(Err(e)) => rsx! {
                        div { class: "text-xs text-muted-foreground", "No review: {e}" }
                    },
                    Some(Ok(None)) => rsx! {
                        EmptyThread {}
                    },
                    Some(Ok(Some(_))) => rsx! {
                        {match &*data.comments.read_unchecked() {
                            Some(Some(Ok(list))) if list.is_empty() => rsx! {
                                EmptyThread {}
                            },
                            Some(Some(Ok(_))) => rsx! {
                                for comment in data.sorted.read().iter().cloned() {
                                    CommentRow {
                                        key: "{comment.id}",
                                        comment,
                                        head_commit: head_hex(),
                                        video_id: video_id.clone(),
                                    }
                                }
                            },
                            Some(Some(Err(e))) => rsx! {
                                div { class: "text-xs text-destructive", "Couldn't read comments: {e}" }
                            },
                            _ => rsx! {
                                Text { variant: TextVariant::Muted, class: "text-xs", "Reading comments…" }
                            },
                        }}
                    },
                }}
            }
            Composer { video_id, watching }
        }
    }
}

#[component]
fn EmptyThread() -> Element {
    rsx! {
        div { class: "flex flex-col items-center gap-2 py-10 text-center",
            div { class: "flex h-12 w-12 items-center justify-center rounded-xl bg-muted/40 text-muted-foreground",
                Pencil { size: 20 }
            }
            span { class: "text-[13px] font-medium", "No comments yet" }
            span { class: "text-xs text-muted-foreground",
                "Leave a comment below to start the review."
            }
        }
    }
}

/// One comment row: avatar, author + badges, timecode chip, body, and
/// (for members) a hover-revealed delete.
#[component]
fn CommentRow(comment: ReviewComment, head_commit: String, video_id: String) -> Element {
    let data = use_context::<ReviewData>();
    let mut draw = DrawCtx::use_ctx();
    let toast = use_toast();
    let mut busy = use_signal(|| false);

    let tc = comment.timecode_secs;
    // Only badge against a KNOWN head — an unloaded chain must not
    // flag every comment as old.
    let stale = !head_commit.is_empty() && comment.commit_id != head_commit;
    let author = display_author(&comment.author);
    let color = avatar_css(&author);
    let pinned = is_pinned(tc);
    let focused = (draw.focused)() == Some(comment.id);

    let focus_row = {
        let video_id = video_id.clone();
        let comment = comment.clone();
        move |_| {
            // A general note has no frame — highlight it without
            // yanking the playhead to 0:00.
            if pinned {
                seek_to(&video_id, tc);
                pause(&video_id);
            }
            draw.focus(&comment);
        }
    };

    let delete = {
        let id = comment.id;
        move |evt: Event<MouseData>| {
            evt.stop_propagation();
            if *busy.peek() {
                return;
            }
            busy.set(true);
            let org = data.org.peek().clone();
            let mut comments = data.comments;
            spawn(async move {
                match remove_comment(&org, id).await {
                    Ok(()) => comments.restart(),
                    Err(e) => {
                        toast.error("Couldn't delete".into(), ToastOptions::new().description(e));
                    }
                }
                busy.set(false);
            });
        }
    };

    rsx! {
        div {
            class: if focused {
                "group/comment cursor-pointer rounded-lg border border-indigo-400/50 bg-muted/30 px-3 py-2.5"
            } else {
                "group/comment cursor-pointer rounded-lg border border-border/40 px-3 py-2.5 hover:border-border hover:bg-muted/20 transition-colors"
            },
            onclick: focus_row,
            div { class: "flex items-start gap-2.5",
                span {
                    class: "mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-full text-[11px] font-bold text-white",
                    style: "background:{color}",
                    title: "{author}",
                    "{initials(&author)}"
                }
                div { class: "min-w-0 flex-1 flex flex-col gap-1",
                    div { class: "flex items-center gap-2 flex-wrap",
                        span { class: "text-[13px] font-semibold truncate", "{author}" }
                        if pinned {
                            button {
                                class: "inline-flex items-center gap-1 rounded-md bg-indigo-500/10 px-1.5 py-0.5 font-mono text-[11px] tabular-nums text-indigo-400 hover:bg-indigo-500/20",
                                title: "Jump to this frame",
                                Clock { size: 10 }
                                "{display_timecode(tc)}"
                            }
                        }
                        if !comment.annotation.is_empty() {
                            span { class: "text-purple-400", title: "Has a drawing — click to view",
                                Pencil { size: 12 }
                            }
                        }
                        if !comment.via_link.is_empty() {
                            // Guest-lane feedback carries the link it
                            // came through (issue #272 AC 1).
                            Badge { variant: BadgeVariant::Outline, "via {comment.via_link}" }
                        }
                        if stale {
                            // Feedback about an older cut stays
                            // attributed to it (AC 2) — never silently
                            // re-anchored.
                            Badge { variant: BadgeVariant::Outline,
                                "on {crate::short_id(&comment.commit_id)}"
                            }
                        }
                    }
                    if !comment.body.is_empty() {
                        span { class: "text-[13px] text-muted-foreground leading-relaxed whitespace-pre-wrap break-words",
                            "{comment.body}"
                        }
                    }
                }
                if task_ui_core::vox_session::guest_share().is_none() {
                    button {
                        class: "shrink-0 flex h-5 w-5 items-center justify-center rounded text-muted-foreground opacity-0 group-hover/comment:opacity-100 hover:text-destructive transition-opacity",
                        title: "Delete comment",
                        disabled: busy(),
                        onclick: delete,
                        X { size: 12 }
                    }
                }
            }
        }
    }
}

/// The composer: name + text, the amber frame pin (detachable — but a
/// pending drawing re-pins it), and send.
#[component]
fn Composer(video_id: String, watching: ReadSignal<Option<String>>) -> Element {
    let data = use_context::<ReviewData>();
    let player = PlayerCtx::use_ctx();
    let mut draw = DrawCtx::use_ctx();
    let toast = use_toast();

    let author = use_signal(String::new);
    let body = use_signal(String::new);
    let mut busy = use_signal(|| false);
    // The frame pin: on by default; a click detaches. A pending
    // drawing overrides to attached — drawings are frame-anchored.
    let mut attach_tc = use_signal(|| true);

    let has_drawing = !draw.pending.read().is_empty();
    let attached = attach_tc() || has_drawing;
    // The chip shows the frame the comment will ACTUALLY pin to: a
    // pending drawing anchors to its first stroke's frame, not the
    // live clock.
    let now = (draw.pending_at)()
        .filter(|_| has_drawing)
        .unwrap_or_else(|| (player.now)());

    let post = move |_| {
        // The recorded version is the one on screen; without a chain
        // yet there is nothing to attribute to.
        let Some(commit_id) = watching.peek().clone() else {
            return;
        };
        let text = body.peek().trim().to_string();
        let strokes = draw.pending.peek().clone();
        // A drawing alone is a valid comment (the server allows text OR
        // strokes); an empty everything is nothing to post.
        if (text.is_empty() && strokes.is_empty()) || *busy.peek() {
            return;
        }
        busy.set(true);
        let org = data.org.peek().clone();
        let (root_id_now, path_now) = (*data.root_id.peek(), data.path.peek().clone());
        let existing = *data.review_id.peek();
        // A drawing pins to the frame its first stroke landed on —
        // NOT wherever the user scrubbed afterwards. A plain comment
        // pins to the current frame, or detaches to the sentinel.
        let timecode_secs = if let Some(at) = (!strokes.is_empty())
            .then(|| *draw.pending_at.peek())
            .flatten()
        {
            at
        } else if *attach_tc.peek() || !strokes.is_empty() {
            *player.now.peek()
        } else {
            UNPINNED
        };
        let comment = NewReviewComment {
            timecode_secs,
            author: author.peek().trim().to_string(),
            body: text,
            commit_id,
            annotation: strokes,
        };
        let (mut body, mut draw, mut scope, mut comments) = (body, draw, data.scope, data.comments);
        spawn(async move {
            // First comment on this file: mint the review now (the one
            // write-on-demand), then post into it.
            let target = match existing {
                Some(id) => Ok(id),
                None => ensure_review(&org, root_id_now, &path_now)
                    .await
                    .map(|r| r.id),
            };
            let posted = match target {
                Ok(id) => post_comment(&org, id, comment).await.map(|_| ()),
                Err(e) => Err(e),
            };
            match posted {
                Ok(()) => {
                    body.set(String::new());
                    draw.pending.set(Vec::new());
                    draw.pending_at.set(None);
                    draw.draw_mode.set(false);
                    if existing.is_none() {
                        scope.restart();
                    }
                    comments.restart();
                }
                Err(e) => {
                    toast.error(
                        "Couldn't comment".into(),
                        ToastOptions::new().description(e),
                    );
                }
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "shrink-0 border-t border-border/40 px-3 py-2.5 flex flex-col gap-2",
            div { class: "rounded-lg border border-border/60 bg-muted/20 px-2 py-1.5 flex items-center gap-2",
                if attached {
                    span { class: "inline-flex shrink-0 items-center gap-1 rounded bg-amber-500/20 px-1.5 py-0.5 font-mono text-[11px] tabular-nums text-amber-400",
                        Clock { size: 10 }
                        "{display_timecode(now)}"
                    }
                }
                Input {
                    value: body,
                    size: InputSize::Small,
                    placeholder: "Leave a comment…".to_string(),
                    class: "flex-1 border-0 bg-transparent",
                }
            }
            div { class: "flex items-center gap-1.5",
                button {
                    class: if attached { "flex h-7 w-7 items-center justify-center rounded-md text-amber-400 bg-amber-400/10" } else { "flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:text-amber-400 hover:bg-amber-400/10" },
                    title: if has_drawing { "A drawing pins the comment to its frame" } else if attached { "Detach from this frame" } else { "Pin to the current frame" },
                    // A drawing forces the pin: the toggle reads
                    // attached and won't detach until it's cleared.
                    onclick: move |_| {
                        if !has_drawing {
                            let now_on = *attach_tc.peek();
                            attach_tc.set(!now_on);
                        }
                    },
                    Clock { size: 16 }
                }
                button {
                    class: if *draw.draw_mode.read() || has_drawing { "flex h-7 w-7 items-center justify-center rounded-md text-purple-400 bg-purple-500/15" } else { "flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:text-purple-400 hover:bg-purple-500/15" },
                    title: "Draw on this frame",
                    onclick: {
                        let video_id = video_id.clone();
                        move |_| {
                            if *draw.draw_mode.peek() {
                                draw.cancel_drawing();
                            } else {
                                pause(&video_id);
                                draw.viewing.set(Vec::new());
                                draw.draw_mode.set(true);
                            }
                        }
                    },
                    Pencil { size: 16 }
                }
                Input {
                    value: author,
                    size: InputSize::Small,
                    placeholder: "Name".to_string(),
                    class: "max-w-28 ml-auto",
                }
                button {
                    class: "flex h-8 w-8 items-center justify-center rounded-full bg-primary text-primary-foreground hover:opacity-90 disabled:opacity-40",
                    title: "Send",
                    // No chain head yet = nothing to attribute a
                    // comment to; disabled beats a silent no-op click.
                    disabled: busy() || watching().is_none(),
                    onclick: post,
                    SendHorizontal { size: 14 }
                }
            }
        }
    }
}
