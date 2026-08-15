//! The timeline band: a thin seek track (buffered layer, indigo
//! gradient fill, hover-grown, draggable playhead) with the signature
//! avatar-dot comment markers in a dedicated row *below* the track —
//! markers never obscure the track, and the track stays 1px calm.
//!
//! Hovering the track floats a filmstrip crop above the cursor — the
//! same sprite the rendition pipeline already produces, windowed to
//! the hovered moment (the reference tool spins up a second decoder
//! for this; the filmstrip is cheaper and already paid for).

use dioxus::prelude::*;
use files_proto::ReviewComment;
use uuid::Uuid;

use super::{
    DrawCtx, PlayerCtx, avatar_css, display_author, display_timecode, element_dims, initials,
    is_pinned, pause, seek_to,
};

/// Preview crop size (CSS px) — 16:9 under the reference's 160px width.
const PREVIEW_W: f64 = 160.0;
const PREVIEW_H: f64 = 90.0;

#[component]
pub(crate) fn TimelineBand(
    video_id: String,
    /// Timecoded comments become marker dots. Already sorted.
    comments: ReadSignal<Vec<ReviewComment>>,
    filmstrip: Option<String>,
    /// Mini chrome drops the marker row when there are no comments
    /// (an empty 24px band under an embed is dead weight).
    mini: bool,
) -> Element {
    let player = PlayerCtx::use_ctx();
    let mut draw = DrawCtx::use_ctx();

    let track_id = use_hook(|| format!("review-track-{}", Uuid::new_v4().simple()));
    let strip_img_id = use_hook(|| format!("review-prev-{}", Uuid::new_v4().simple()));

    let mut track_w = use_signal(|| 0.0f64);
    let mut dragging = use_signal(|| false);
    // Hover preview state: cursor x within the track, or None.
    let mut hover_x = use_signal(|| Option::<f64>::None);
    // The filmstrip's rendered width at PREVIEW_H, measured once per
    // hover session (naturalWidth × PREVIEW_H / naturalHeight).
    let mut strip_w = use_signal(|| 0.0f64);
    // The marker whose hover card is open.
    let mut hovered_marker = use_signal(|| Option::<Uuid>::None);

    let duration = (player.duration)().max(0.0);
    let frac = |secs: f64| {
        if duration > 0.0 {
            (secs / duration).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let played_pct = frac((player.now)()) * 100.0;
    let buffered_pct = frac((player.buffered)()) * 100.0;

    let seek_at = {
        let video_id = video_id.clone();
        move |x: f64| {
            let w = *track_w.peek();
            if w > 0.0 && duration > 0.0 {
                seek_to(&video_id, (x / w).clamp(0.0, 1.0) * duration);
            }
        }
    };

    // Only pinned feedback becomes a dot — a general note has no frame.
    let markers: Vec<ReviewComment> = comments()
        .into_iter()
        .filter(|c| is_pinned(c.timecode_secs))
        .collect();
    let show_marker_row = !mini || !markers.is_empty();

    // Hover preview geometry.
    let preview = hover_x().and_then(|x| {
        let w = track_w();
        if w <= 0.0 || duration <= 0.0 || filmstrip.is_none() {
            return None;
        }
        let f = (x / w).clamp(0.0, 1.0);
        let sw = strip_w();
        // Center the crop window on the hovered frame, clamped to the
        // sprite's ends.
        let offset = if sw > PREVIEW_W {
            (f * sw - PREVIEW_W / 2.0).clamp(0.0, sw - PREVIEW_W)
        } else {
            0.0
        };
        let left = x.clamp(PREVIEW_W / 2.0, (w - PREVIEW_W / 2.0).max(PREVIEW_W / 2.0));
        Some((left, offset, f * duration))
    });

    rsx! {
        div { class: "shrink-0 px-3 pt-1 pb-0.5 select-none",
            // ── track ───────────────────────────────────────────
            div {
                id: track_id.clone(),
                class: "group/track relative py-1.5 cursor-pointer",
                onpointerenter: {
                    let track_id = track_id.clone();
                    let strip_img_id = strip_img_id.clone();
                    let has_strip = filmstrip.is_some();
                    move |_| {
                        let track_id = track_id.clone();
                        let strip_img_id = strip_img_id.clone();
                        spawn(async move {
                            track_w.set(element_dims(&track_id).await.0);
                            if has_strip {
                                // The crop math needs the sprite's
                                // aspect — measure its natural size.
                                let mut e = dioxus::document::eval(&format!(
                                    "var i=document.getElementById('{strip_img_id}');\
                                     if(i&&i.naturalHeight>0){{dioxus.send(i.naturalWidth*{PREVIEW_H}/i.naturalHeight);}}\
                                     else{{dioxus.send(0);}}"
                                ));
                                strip_w.set(e.recv::<f64>().await.unwrap_or(0.0));
                            }
                        });
                    }
                },
                onpointerdown: {
                    // Measure BEFORE the first seek: on touch there is
                    // no hover, so pointerenter's async measure hasn't
                    // landed; and a cached width goes stale across
                    // resize/fullscreen.
                    let seek_at = seek_at.clone();
                    let track_id = track_id.clone();
                    move |evt: Event<PointerData>| {
                        dragging.set(true);
                        let x = evt.data().element_coordinates().x;
                        let (seek_at, track_id) = (seek_at.clone(), track_id.clone());
                        spawn(async move {
                            track_w.set(element_dims(&track_id).await.0);
                            seek_at(x);
                        });
                    }
                },
                onpointermove: {
                    let seek_at = seek_at.clone();
                    move |evt: Event<PointerData>| {
                        let x = evt.data().element_coordinates().x;
                        hover_x.set(Some(x));
                        if *dragging.peek() {
                            seek_at(x);
                        }
                    }
                },
                onpointerup: move |_| dragging.set(false),
                onpointerleave: move |_| {
                    dragging.set(false);
                    hover_x.set(None);
                },
                div { class: "relative h-1 group-hover/track:h-1.5 rounded-full bg-border/60 overflow-visible transition-all",
                    // Buffered ahead of played.
                    div {
                        class: "absolute inset-y-0 left-0 rounded-full bg-border",
                        style: "width:{buffered_pct}%",
                    }
                    // The indigo signature fill.
                    div {
                        class: "absolute inset-y-0 left-0 rounded-full bg-gradient-to-r from-indigo-500 to-indigo-400",
                        style: "width:{played_pct}%",
                    }
                    // Playhead thumb, revealed on hover.
                    div {
                        class: "absolute top-1/2 w-3 h-3 rounded-full bg-indigo-400 shadow-lg opacity-0 group-hover/track:opacity-100 pointer-events-none z-10 transition-opacity",
                        style: "left:{played_pct}%;transform:translate(-50%,-50%)",
                    }
                }
                // ── filmstrip hover preview ─────────────────────
                if let (Some((left, offset, at)), Some(strip)) = (preview, filmstrip.clone()) {
                    div {
                        class: "absolute bottom-full mb-2 z-30 pointer-events-none flex flex-col items-center",
                        style: "left:{left}px;transform:translateX(-50%)",
                        div {
                            class: "rounded-md overflow-hidden border border-border shadow-2xl bg-black",
                            style: "width:{PREVIEW_W}px;height:{PREVIEW_H}px",
                            img {
                                id: strip_img_id.clone(),
                                src: strip,
                                class: "max-w-none",
                                style: "height:{PREVIEW_H}px;transform:translateX(-{offset}px)",
                            }
                        }
                        span { class: "mt-1 rounded-md bg-black/90 px-2 py-0.5 font-mono text-[11px] tabular-nums text-white",
                            "{display_timecode(at)}"
                        }
                    }
                } else if let Some(strip) = filmstrip.clone() {
                    // Keep the sprite in the DOM (hidden) so its natural
                    // size is measurable the moment a hover starts.
                    img {
                        id: strip_img_id.clone(),
                        src: strip,
                        class: "hidden",
                    }
                }
            }
            // ── marker row ──────────────────────────────────────
            if show_marker_row {
                div { class: "relative h-6 mt-0.5",
                    for comment in markers.iter().cloned() {
                        {
                            let pct = frac(comment.timecode_secs) * 100.0;
                            let author = display_author(&comment.author);
                            let color = avatar_css(&author);
                            let focused = (draw.focused)() == Some(comment.id);
                            let hovered = hovered_marker() == Some(comment.id);
                            let id = comment.id;
                            rsx! {
                                div {
                                    key: "{comment.id}",
                                    class: "absolute top-0",
                                    style: "left:{pct}%;transform:translateX(-50%)",
                                    button {
                                        class: if focused {
                                            "flex h-5 w-5 items-center justify-center rounded-full border-2 border-indigo-400 text-[9px] font-bold text-white shadow-md scale-125 ring-2 ring-indigo-400/40 transition-transform"
                                        } else {
                                            "flex h-5 w-5 items-center justify-center rounded-full border-2 border-background text-[9px] font-bold text-white shadow-md hover:scale-110 transition-transform"
                                        },
                                        style: "background:{color}",
                                        onpointerenter: move |_| hovered_marker.set(Some(id)),
                                        onpointerleave: move |_| hovered_marker.set(None),
                                        onclick: {
                                            let video_id = video_id.clone();
                                            let comment = comment.clone();
                                            move |_| {
                                                // A marker is frame feedback:
                                                // land on the frame, hold it,
                                                // reveal the drawing + row.
                                                seek_to(&video_id, comment.timecode_secs);
                                                pause(&video_id);
                                                draw.focus(&comment);
                                            }
                                        },
                                        "{initials(&author)}"
                                    }
                                    // Hover card, clamped by CSS so edge
                                    // markers stay readable.
                                    if hovered {
                                        div {
                                            class: "absolute bottom-full mb-2 w-60 -translate-x-1/2 left-1/2 z-40 pointer-events-none rounded-lg border border-border bg-card p-3 shadow-2xl",
                                            div { class: "flex items-center gap-2",
                                                span {
                                                    class: "flex h-5 w-5 items-center justify-center rounded-full text-[9px] font-bold text-white",
                                                    style: "background:{color}",
                                                    "{initials(&author)}"
                                                }
                                                span { class: "text-[13px] font-medium truncate", "{author}" }
                                                span { class: "ml-auto rounded bg-indigo-500/10 px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-indigo-400",
                                                    "{display_timecode(comment.timecode_secs)}"
                                                }
                                            }
                                            if !comment.body.is_empty() {
                                                p { class: "mt-1.5 text-xs text-muted-foreground line-clamp-2 text-left",
                                                    "{comment.body}"
                                                }
                                            }
                                            if !comment.annotation.is_empty() {
                                                span { class: "mt-1 inline-block text-[10px] text-purple-400", "✏ drawing" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
