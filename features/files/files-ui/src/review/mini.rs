//! [`MiniPlayer`] — the embeddable player for file pages: stage +
//! timeline (markers included, they're the point) + slim transport,
//! and an expand button that opens the full [`ReviewScreen`] as a
//! fixed overlay. The conversation lives in the full screen; the mini
//! shows where it lands.
//!
//! Expanded and collapsed are EITHER/OR mounts: the mini body
//! unmounts under the overlay, so there is never a second `<video>`
//! holding decoded media, a second poll interval, or a second event
//! stream running behind the full screen. Collapsing re-resolves one
//! server-side-cached rendition RPC — cheap.

use architect_ui::lucide_dioxus::Maximize2;
use dioxus::prelude::*;
use uuid::Uuid;

use super::progress::TimelineBand;
use super::stage::VideoStage;
use super::transport::TransportBar;
use super::{DrawCtx, PlayerCtx, ReviewScreen, resolve_sources, use_review_data};

/// Props are `ReadSignal`s so the resource re-resolves when the mount
/// is reused with a different file (a root swap can hand this
/// instance a new `(root_id, path)` in place — plain props would keep
/// streaming the old file's proxy).
#[component]
pub fn MiniPlayer(
    org: ReadSignal<String>,
    root_id: ReadSignal<Uuid>,
    path: ReadSignal<String>,
    /// Expansion override: a host with a UNIFIED media session (the
    /// app shell's dock ⇄ zoom system) opens the full screen there
    /// instead of this mount opening a private overlay — one playback
    /// surface, not one per embed. `None` = self-contained, as ever.
    #[props(default)]
    on_expand: Option<EventHandler<()>>,
) -> Element {
    let mut expanded = use_signal(|| false);

    rsx! {
        if expanded() {
            div { class: "fixed inset-0 z-50",
                ReviewScreen {
                    org,
                    root_id,
                    path,
                    on_close: move |()| expanded.set(false),
                }
            }
        } else {
            MiniBody {
                org,
                root_id,
                path,
                on_expand: move |()| match &on_expand {
                    Some(host) => host.call(()),
                    None => expanded.set(true),
                },
            }
        }
    }
}

#[component]
fn MiniBody(
    org: ReadSignal<String>,
    root_id: ReadSignal<Uuid>,
    path: ReadSignal<String>,
    on_expand: EventHandler<()>,
) -> Element {
    let video_id = use_hook(|| format!("review-mini-{}", Uuid::new_v4().simple()));
    let stage_id = use_hook(|| format!("review-ministage-{}", Uuid::new_v4().simple()));
    let container_id = use_hook(|| format!("review-minibox-{}", Uuid::new_v4().simple()));

    let player = use_hook(|| PlayerCtx::install(&video_id, &stage_id, &container_id));
    use_context_provider(|| player);
    // The mini never draws, but the stage + timeline read the draw
    // context (focus, viewing) — install an inert one.
    let draw = use_hook(DrawCtx::install);
    use_context_provider(|| draw);
    let data = use_review_data(org, root_id, path);
    use_context_provider(|| data);

    let sources = use_resource(move || {
        let (org, root_id, path) = (org(), root_id(), path());
        async move { resolve_sources(&org, root_id, &path, None).await }
    });

    rsx! {
        div {
            id: container_id.clone(),
            class: "flex flex-col overflow-hidden rounded-md border border-border/40 bg-card",
            {match &*sources.read_unchecked() {
                None => rsx! {
                    div { class: "flex aspect-video max-h-96 items-center justify-center bg-black",
                        div { class: "h-8 w-8 animate-spin rounded-full border-4 border-white/20 border-t-white" }
                    }
                },
                Some(Err(e)) => rsx! {
                    // A failed proxy must not hide the conversation:
                    // the full screen still shows the comment rail, so
                    // the door stays open.
                    div { class: "flex items-center gap-2 px-3 py-2",
                        span { class: "min-w-0 flex-1 truncate text-xs text-muted-foreground",
                            "No proxy rendition: {e}"
                        }
                        button {
                            class: "flex h-7 shrink-0 items-center gap-1.5 rounded-md bg-muted/50 px-2 text-xs hover:bg-muted/80",
                            title: "Open the review conversation",
                            onclick: move |_| on_expand.call(()),
                            Maximize2 { size: 12 }
                            "Review"
                        }
                    }
                },
                Some(Ok(src)) => rsx! {
                    div { class: "relative",
                        VideoStage {
                            stage_id: stage_id.clone(),
                            video_id: video_id.clone(),
                            src: src.proxy.clone(),
                            mini: true,
                            poster: src.filmstrip.clone(),
                            waveform: src.peaks.clone(),
                        }
                        // The door to the full experience.
                        button {
                            class: "absolute right-2 top-2 flex h-7 items-center gap-1.5 rounded-md bg-black/60 px-2 text-xs text-white opacity-80 hover:opacity-100",
                            title: "Open the full review",
                            onclick: move |_| on_expand.call(()),
                            Maximize2 { size: 12 }
                            "Review"
                        }
                    }
                    TimelineBand {
                        video_id: video_id.clone(),
                        comments: ReadSignal::from(data.sorted),
                        filmstrip: src.filmstrip.clone(),
                        mini: true,
                    }
                    TransportBar {
                        video_id: video_id.clone(),
                        container_id: container_id.clone(),
                        mini: true,
                    }
                },
            }}
        }
    }
}
