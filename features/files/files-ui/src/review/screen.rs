//! [`ReviewScreen`] — the full review experience: top bar, stage,
//! timeline, transport, comment rail, compare.
//!
//! The screen fills whatever box it's given (`h-full w-full`): the
//! guest entry gives it the viewport; the files page opens it as a
//! fixed overlay from the mini player. It never scrolls as a page —
//! only the comment thread scrolls.

use architect_ui::lucide_dioxus::{ChevronDown, Columns2, Link as LinkIcon, X};
use architect_ui::prelude::*;
use dioxus::prelude::*;
use uuid::Uuid;

use super::panel::CommentsRail;
use super::progress::TimelineBand;
use super::stage::VideoStage;
use super::transport::TransportBar;
use super::{
    DrawCtx, PlayerCtx, resolve_sources, seek_by, toggle_fullscreen, toggle_play, use_review_data,
    video_op,
};
use files_proto::FilesEvent;

/// The full-screen review surface.
#[component]
pub fn ReviewScreen(
    org: ReadSignal<String>,
    root_id: ReadSignal<Uuid>,
    path: ReadSignal<String>,
    /// Present when the screen is an overlay someone can leave (the
    /// files page); absent on dedicated pages (the guest entry).
    on_close: Option<EventHandler<()>>,
    /// Media-element id override, for a host that OWNS the playback
    /// element's identity beyond this mount (the app shell's unified
    /// media session drives dock chrome against the same id). `None` =
    /// a private per-mount id, as ever.
    #[props(default)]
    element_id: Option<String>,
) -> Element {
    // Stable per-mount element ids — the eval seams address the live
    // elements by id, and two open files must not cross wires.
    let container_id = use_hook(|| format!("review-screen-{}", Uuid::new_v4().simple()));
    let video_id = use_hook(move || {
        element_id.unwrap_or_else(|| format!("review-video-{}", Uuid::new_v4().simple()))
    });
    let stage_id = use_hook(|| format!("review-stage-{}", Uuid::new_v4().simple()));
    let cmp_video_id = use_hook(|| format!("review-cmp-{}", Uuid::new_v4().simple()));

    let player = use_hook(|| PlayerCtx::install(&video_id, &stage_id, &container_id));
    use_context_provider(|| player);
    let draw = use_hook(DrawCtx::install);
    use_context_provider(|| draw);
    let data = use_review_data(org, root_id, path);
    use_context_provider(|| data);

    // ── versions (issue #270 AC 4) ─────────────────────────────
    // The pinned version (`None` follows the head) and an optional
    // second version compared side by side. A pin carries
    // `(commit, path)` — the chain follows renames, so a pre-rename
    // version resolves under the path it lived at.
    let versions = use_resource(move || {
        let (org, root_id, path) = (org(), root_id(), path());
        async move { crate::fetch_chain(&org, root_id, path).await }
    });
    let selected = use_signal(|| Option::<(String, String)>::None);
    let mut compare = use_signal(|| Option::<(String, String)>::None);
    let head_commit = use_memo(move || match &*versions.read() {
        Some(Ok(chain)) => chain.first().map(|e| e.commit_id.clone()),
        _ => None,
    });
    // What the main player is actually showing — what a fresh comment
    // records as its file version.
    let watching = use_memo(move || selected().map(|(commit, _)| commit).or(head_commit()));

    let sources = use_resource(move || {
        let (org, root_id, path, pin) = (org(), root_id(), path(), selected());
        async move {
            match pin {
                Some((commit, vpath)) => resolve_sources(&org, root_id, &vpath, Some(commit)).await,
                None => resolve_sources(&org, root_id, &path, None).await,
            }
        }
    });
    let compare_sources = use_resource(move || {
        let (org, root_id, pin) = (org(), root_id(), compare());
        async move {
            match pin {
                Some((commit, vpath)) => {
                    Some(resolve_sources(&org, root_id, &vpath, Some(commit)).await)
                }
                None => None,
            }
        }
    });

    // A checkpoint anywhere on this root moves the head: re-read the
    // chain (so the switcher shows the new version and fresh comments
    // record the right commit) and, when following the head,
    // re-resolve the stream.
    crate::use_files_events(
        move || org(),
        move |event: FilesEvent| {
            let (mut versions, mut sources) = (versions, sources);
            if let FilesEvent::Checkpointed(info) = &event
                && info.root_id == *root_id.peek()
            {
                versions.restart();
                if selected.peek().is_none() {
                    sources.restart();
                }
            }
        },
    );

    // ── review keyboard (reference contract) ───────────────────
    // Space/K play-pause, ←/→ ∓5s, J/L ∓10s, M mute, F fullscreen.
    // Suppressed while typing and while the pen owns the stage.
    {
        let (container_id, video_id) = (container_id.clone(), video_id.clone());
        use_hook(move || {
            spawn(async move {
                let mut chan = dioxus::document::eval(&format!(
                    "var h=function(e){{\
                       var t=e.target,tag=((t&&t.tagName)||'').toLowerCase();\
                       if(tag==='input'||tag==='textarea'||(t&&t.isContentEditable))return;\
                       var k=e.key;\
                       if(k===' '){{e.preventDefault();}}\
                       dioxus.send(k);\
                     }};\
                     document.addEventListener('keydown',h);\
                     var g=setInterval(function(){{\
                       if(!document.getElementById('{container_id}')){{\
                         document.removeEventListener('keydown',h);clearInterval(g);}}\
                     }},1000);"
                ));
                while let Ok(key) = chan.recv::<String>().await {
                    let draw = draw;
                    if *draw.draw_mode.peek() {
                        continue;
                    }
                    match key.as_str() {
                        " " | "k" | "K" => toggle_play(&video_id),
                        "ArrowLeft" => seek_by(&video_id, -5.0),
                        "ArrowRight" => seek_by(&video_id, 5.0),
                        "j" | "J" => seek_by(&video_id, -10.0),
                        "l" | "L" => seek_by(&video_id, 10.0),
                        "m" | "M" => video_op(&video_id, "v.muted=!v.muted;"),
                        "f" | "F" => toggle_fullscreen(&container_id),
                        _ => {}
                    }
                }
            });
        });
    }

    let mut rail_open = use_signal(|| true);
    let file_name = use_memo(move || {
        let p = path();
        p.rsplit('/').next().unwrap_or(&p).to_string()
    });

    let comparing = compare().is_some();

    rsx! {
        div {
            id: container_id.clone(),
            class: "flex h-full w-full flex-col overflow-hidden bg-background text-foreground",
            // ── top bar ─────────────────────────────────────────
            div { class: "flex h-12 shrink-0 items-center gap-2 border-b border-border/60 bg-card px-3",
                if let Some(close) = on_close {
                    button {
                        class: "flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:text-foreground hover:bg-muted/50",
                        title: "Close review",
                        onclick: move |_| close.call(()),
                        X { size: 16 }
                    }
                }
                span { class: "min-w-0 truncate text-[13px] font-medium", "{file_name}" }
                if task_ui_core::vox_session::guest_share().is_some() {
                    Badge { variant: BadgeVariant::Secondary, "shared review" }
                }
                div { class: "flex-1" }
                VersionMenu {
                    versions,
                    selected,
                    compare,
                    watching,
                }
                ShareReviewButton {}
                button {
                    class: if rail_open() { "flex h-7 w-7 items-center justify-center rounded-md text-indigo-400 bg-indigo-500/10" } else { "flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:text-foreground hover:bg-muted/50" },
                    title: "Toggle comments",
                    onclick: move |_| {
                        let open = *rail_open.peek();
                        rail_open.set(!open);
                    },
                    Columns2 { size: 16 }
                }
            }
            // ── body ────────────────────────────────────────────
            div { class: "relative flex flex-1 min-h-0 overflow-hidden",
                div { class: "flex min-w-0 flex-1 flex-col",
                    {match &*sources.read_unchecked() {
                        None => rsx! {
                            div { class: "flex flex-1 items-center justify-center bg-black",
                                div { class: "h-10 w-10 animate-spin rounded-full border-4 border-white/20 border-t-white" }
                            }
                        },
                        Some(Err(e)) => rsx! {
                            // No transcoder, or the file yields no proxy —
                            // the screen degrades to a note, never a
                            // broken <video>.
                            div { class: "flex flex-1 items-center justify-center bg-black",
                                span { class: "text-sm text-muted-foreground", "No proxy rendition: {e}" }
                            }
                        },
                        Some(Ok(src)) => rsx! {
                            // One stable tree whether comparing or not —
                            // a shape change would remount the A stage
                            // and reset its clock mid-review.
                            div { class: "flex flex-1 min-h-0",
                                div { class: "relative flex min-w-0 flex-1 flex-col",
                                    VideoStage {
                                        stage_id: stage_id.clone(),
                                        video_id: video_id.clone(),
                                        src: src.proxy.clone(),
                                        mini: false,
                                        // No poster here: the full stage
                                        // letterboxes, and a background
                                        // crop can't match the contained
                                        // picture box.
                                        waveform: src.peaks.clone(),
                                    }
                                    if comparing {
                                        span { class: "absolute left-2 top-2 rounded bg-sky-500/90 px-1.5 py-0.5 text-[11px] font-semibold text-white",
                                            "A"
                                        }
                                    }
                                }
                                if comparing {
                                    div { class: "w-px bg-border" }
                                    ComparePane {
                                        cmp_video_id: cmp_video_id.clone(),
                                        video_id: video_id.clone(),
                                        compare_sources,
                                        on_close: move |()| compare.set(None),
                                    }
                                }
                            }
                            TimelineBand {
                                video_id: video_id.clone(),
                                comments: ReadSignal::from(data.sorted),
                                filmstrip: src.filmstrip.clone(),
                                mini: false,
                            }
                            TransportBar {
                                video_id: video_id.clone(),
                                container_id: container_id.clone(),
                                mini: false,
                            }
                        },
                    }}
                }
                // The rail: fixed 360px column at md+; a full-width
                // overlay sheet over the stage on mobile.
                if rail_open() {
                    div { class: "absolute inset-y-0 right-0 z-20 flex w-full flex-col border-l border-border/60 bg-card md:static md:inset-auto md:w-[360px] md:shrink-0",
                        CommentsRail {
                            video_id: video_id.clone(),
                            watching: ReadSignal::from(watching),
                            head: ReadSignal::from(head_commit),
                        }
                    }
                }
            }
        }
    }
}

/// The compared version's pane: side B, slaved to A's clock (A stays
/// the audible master; a slaved pane that also played sound would
/// double the track).
#[component]
fn ComparePane(
    cmp_video_id: String,
    video_id: String,
    compare_sources: Resource<Option<Result<super::Sources, String>>>,
    on_close: EventHandler<()>,
) -> Element {
    // The pane div anchors the sync loop's lifetime: B's <video>
    // mounts late (after its rendition resolves), so a missing B is a
    // skipped frame, never a shutdown — the loop ends only when the
    // pane itself unmounts.
    let pane_id = use_hook(|| format!("review-cmppane-{}", uuid::Uuid::new_v4().simple()));

    // B follows A — playing when A plays, seeking when drift exceeds
    // 50ms (one frame's tolerance while paused avoids decoder thrash).
    {
        let (a, b, pane) = (video_id.clone(), cmp_video_id.clone(), pane_id.clone());
        use_hook(move || {
            let _ = dioxus::document::eval(&format!(
                "var loop=function(){{\
                   if(!document.getElementById('{pane}')){{return;}}\
                   var a=document.getElementById('{a}'),b=document.getElementById('{b}');\
                   if(a&&b){{\
                     b.muted=true;\
                     if(!a.paused){{\
                       if(b.paused){{b.play();}}\
                       if(Math.abs(b.currentTime-a.currentTime)>0.05){{b.currentTime=a.currentTime;}}\
                     }}else{{\
                       if(!b.paused){{b.pause();}}\
                       if(Math.abs(b.currentTime-a.currentTime)>(1/24)){{b.currentTime=a.currentTime;}}\
                     }}\
                   }}\
                   requestAnimationFrame(loop);\
                 }};requestAnimationFrame(loop);"
            ));
        });
    }

    rsx! {
        div {
            id: pane_id.clone(),
            class: "relative flex min-w-0 flex-1 flex-col bg-black",
            {match &*compare_sources.read_unchecked() {
                Some(Some(Ok(cmp))) => rsx! {
                    video {
                        id: cmp_video_id.clone(),
                        src: cmp.proxy.clone(),
                        preload: "metadata",
                        muted: true,
                        playsinline: true,
                        class: "absolute inset-0 h-full w-full object-contain",
                    }
                },
                Some(Some(Err(e))) => rsx! {
                    div { class: "flex flex-1 items-center justify-center",
                        span { class: "text-sm text-muted-foreground", "No proxy for that version: {e}" }
                    }
                },
                _ => rsx! {
                    div { class: "flex flex-1 items-center justify-center",
                        div { class: "h-8 w-8 animate-spin rounded-full border-4 border-white/20 border-t-white" }
                    }
                },
            }}
            span { class: "absolute right-8 top-2 rounded bg-emerald-500/90 px-1.5 py-0.5 text-[11px] font-semibold text-white",
                "B"
            }
            button {
                class: "absolute right-2 top-2 flex h-6 w-6 items-center justify-center rounded bg-black/50 text-white hover:bg-black/70",
                title: "Close compare",
                onclick: move |_| on_close.call(()),
                X { size: 12 }
            }
        }
    }
}

/// The version pill + menu: watch any version in the chain, arm the
/// compare pane with another.
#[component]
fn VersionMenu(
    versions: Resource<Result<Vec<files_proto::ChainEntry>, String>>,
    selected: Signal<Option<(String, String)>>,
    compare: Signal<Option<(String, String)>>,
    watching: Memo<Option<String>>,
) -> Element {
    let mut open = use_signal(|| false);

    let chain = match &*versions.read_unchecked() {
        Some(Ok(chain)) if chain.len() > 1 => chain.clone(),
        _ => return rsx! {},
    };
    let total = chain.len();
    let current_n = chain
        .iter()
        .position(|e| Some(&e.commit_id) == watching.read().as_ref())
        .map_or(total, |i| total - i);

    rsx! {
        div { class: "relative",
            button {
                class: "flex items-center gap-1 rounded-md bg-primary px-2.5 py-1 text-xs font-medium text-primary-foreground hover:opacity-90",
                onclick: move |_| {
                    let o = *open.peek();
                    open.set(!o);
                },
                "v{current_n}"
                ChevronDown { size: 12 }
            }
            if open() {
                div { class: "absolute right-0 top-full z-50 mt-1 min-w-[200px] rounded-xl border border-border bg-card py-1.5 shadow-2xl",
                    for (i , entry) in chain.iter().cloned().enumerate() {
                        {
                            let n = total - i;
                            let is_watched = *watching.read() == Some(entry.commit_id.clone());
                            let is_compared = compare.read().as_ref().map(|(c, _)| c)
                                == Some(&entry.commit_id);
                            let pin = (entry.commit_id.clone(), entry.path.clone());
                            let pin_cmp = pin.clone();
                            rsx! {
                                div {
                                    key: "{entry.commit_id}",
                                    class: "mx-1 flex items-center gap-2 rounded-lg px-2.5 py-1.5",
                                    button {
                                        class: if is_watched { "flex flex-1 items-center gap-2 text-left text-sm text-indigo-400 font-medium" } else { "flex flex-1 items-center gap-2 text-left text-sm hover:text-foreground text-muted-foreground" },
                                        title: "{entry.commit_id}",
                                        onclick: move |_| {
                                            // Head stays "follow the head",
                                            // not a pin.
                                            if i == 0 {
                                                selected.set(None);
                                            } else {
                                                selected.set(Some(pin.clone()));
                                            }
                                            open.set(false);
                                        },
                                        span { class: "font-mono tabular-nums", "v{n}" }
                                        // A curated Named Version is the
                                        // thing a client compares — its
                                        // stars ride the row.
                                        for nm in entry.names.iter() {
                                            span { class: "truncate text-xs", "★ {nm}" }
                                        }
                                    }
                                    button {
                                        class: if is_compared { "rounded bg-emerald-500/20 px-1.5 py-0.5 text-xs text-emerald-400" } else { "rounded px-1.5 py-0.5 text-xs text-muted-foreground hover:bg-muted/50" },
                                        title: "Compare side by side",
                                        onclick: move |_| {
                                            let same = compare.peek().as_ref().map(|(c, _)| c)
                                                == Some(&pin_cmp.0);
                                            if same {
                                                compare.set(None);
                                            } else {
                                                compare.set(Some(pin_cmp.clone()));
                                            }
                                            open.set(false);
                                        },
                                        "⇆"
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

/// Mint the guest link (issue #272): an anonymous reviewer gets this
/// review — comment capability on — and nothing else in the org.
/// Members only; the guest lane denies minting.
#[component]
fn ShareReviewButton() -> Element {
    let data = use_context::<super::ReviewData>();
    let toast = use_toast();
    let mut busy = use_signal(|| false);

    if task_ui_core::vox_session::guest_share().is_some() {
        return rsx! {};
    }
    let Some(review_id) = (data.review_id)() else {
        // Nothing to share until the first comment mints the review.
        return rsx! {};
    };

    rsx! {
        button {
            class: "flex h-7 items-center gap-1.5 rounded-md px-2 text-xs text-muted-foreground hover:text-foreground hover:bg-muted/50",
            disabled: busy(),
            title: "Copy a guest review link",
            onclick: move |_| {
                if *busy.peek() {
                    return;
                }
                busy.set(true);
                let org = data.org.peek().clone();
                spawn(async move {
                    let target = share_proto::ShareTarget::Review { id: review_id };
                    let caps = share_proto::ShareCapabilities {
                        comment: true,
                        download: false,
                        file_request: false,
                    };
                    match crate::mint_share_link(&org, target, Some(caps)).await {
                        Ok(url) => {
                            crate::copy_to_clipboard(&url);
                            toast.success(
                                "Review link minted".into(),
                                ToastOptions::new().description(format!("Copied: {url}")),
                            );
                        }
                        Err(e) => {
                            toast.error(
                                "Couldn't mint link".into(),
                                ToastOptions::new().description(e),
                            );
                        }
                    }
                    busy.set(false);
                });
            },
            LinkIcon { size: 14 }
            "Share"
        }
    }
}
