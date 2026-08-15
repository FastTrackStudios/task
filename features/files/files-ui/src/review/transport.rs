//! The transport bar (48px) and the draw toolbar that replaces it in
//! draw mode.
//!
//! Left cluster: play/pause, loop, speed cycle, mute. Center: the
//! timecode chip — clicking it cycles Standard → SMPTE → Frames.
//! Right cluster: annotate (purple when armed) and fullscreen.
//! Every control is a 28px hit target with a 16px icon; every numeral
//! is mono + tabular.

use architect_ui::lucide_dioxus::{
    Maximize, Pause as PauseIcon, Pencil, Play, Repeat, Undo2, Volume2, VolumeX, X,
};
use dioxus::prelude::*;

use super::{DrawCtx, PlayerCtx, TimeFormat, pause, toggle_fullscreen, toggle_play, video_op};

/// One playback-rate click cycles through the reference list.
const SPEED_OPTIONS: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];

const BTN: &str = "flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground \
                   hover:text-foreground hover:bg-muted/50 transition-colors";
const BTN_ACTIVE: &str = "flex h-7 w-7 items-center justify-center rounded-md text-indigo-400 \
                          bg-indigo-500/10 transition-colors";

#[component]
pub(crate) fn TransportBar(
    video_id: String,
    container_id: String,
    /// Mini chrome: no fullscreen (the expand button lives in the mini
    /// header instead), tighter padding.
    mini: bool,
) -> Element {
    let player = PlayerCtx::use_ctx();
    let mut draw = DrawCtx::use_ctx();
    let mut format = use_signal(TimeFormat::default);

    if *draw.draw_mode.read() {
        return rsx! {
            DrawToolbar { video_id }
        };
    }

    let paused = (player.paused)();
    let muted = (player.muted)();
    let looped = (player.looped)();
    let rate = (player.rate)();

    rsx! {
        div { class: if mini {
                "flex h-10 shrink-0 items-center gap-1.5 px-2 bg-card/80 border-t border-border/40"
            } else {
                "flex h-12 shrink-0 items-center gap-2 px-4 bg-card/80 border-t border-border/40"
            },
            // ── left cluster ────────────────────────────────────
            button {
                class: "flex h-7 w-7 items-center justify-center rounded-md text-foreground hover:bg-muted/50",
                title: if paused { "Play (Space)" } else { "Pause (Space)" },
                onclick: {
                    let video_id = video_id.clone();
                    move |_| toggle_play(&video_id)
                },
                if paused {
                    Play { size: 16 }
                } else {
                    PauseIcon { size: 16 }
                }
            }
            button {
                class: if looped { BTN_ACTIVE } else { BTN },
                title: "Loop",
                onclick: {
                    let video_id = video_id.clone();
                    let mut looped_sig = player.looped;
                    move |_| {
                        let on = !*looped_sig.peek();
                        looped_sig.set(on);
                        video_op(&video_id, &format!("v.loop={on};"));
                    }
                },
                Repeat { size: 16 }
            }
            button {
                class: "flex h-7 items-center rounded-md px-1.5 text-xs font-medium tabular-nums text-muted-foreground hover:text-foreground hover:bg-muted/50",
                title: "Playback speed",
                onclick: {
                    let video_id = video_id.clone();
                    move |_| {
                        let current = *player.rate.peek();
                        let idx = SPEED_OPTIONS
                            .iter()
                            .position(|r| (r - current).abs() < 0.01)
                            .unwrap_or(2);
                        let next = SPEED_OPTIONS[(idx + 1) % SPEED_OPTIONS.len()];
                        video_op(&video_id, &format!("v.playbackRate={next};"));
                    }
                },
                "{rate}x"
            }
            button {
                class: if muted { BTN_ACTIVE } else { BTN },
                title: "Mute (M)",
                onclick: {
                    let video_id = video_id.clone();
                    move |_| video_op(&video_id, "v.muted=!v.muted;")
                },
                if muted {
                    VolumeX { size: 16 }
                } else {
                    Volume2 { size: 16 }
                }
            }
            // ── center: the clock chip ──────────────────────────
            div { class: "flex-1 flex justify-center min-w-0",
                button {
                    class: "rounded-md bg-muted/50 px-3 py-1 font-mono text-sm tabular-nums tracking-wide hover:bg-muted/80",
                    title: "Click to switch time format",
                    onclick: move |_| {
                        let next = format.peek().next();
                        format.set(next);
                    },
                    "{format().render((player.now)(), (player.duration)())}"
                }
            }
            // ── right cluster ───────────────────────────────────
            if !mini {
            button {
                class: if !draw.pending.read().is_empty() { "flex h-7 w-7 items-center justify-center rounded-md text-purple-400 bg-purple-500/15" } else { "flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:text-purple-400 hover:bg-purple-500/15 transition-colors" },
                title: "Draw on this frame",
                onclick: {
                    let video_id = video_id.clone();
                    move |_| {
                        // Drawing pins a frame: hold it before arming
                        // the pen.
                        pause(&video_id);
                        draw.draw_mode.set(true);
                        draw.viewing.set(Vec::new());
                    }
                },
                Pencil { size: 16 }
            }
            }
            if !mini {
                button {
                    class: BTN,
                    title: "Fullscreen (F)",
                    onclick: move |_| toggle_fullscreen(&container_id),
                    Maximize { size: 16 }
                }
            }
        }
    }
}

/// Draw mode's replacement bar: the four reference colors, undo,
/// clear, and the exit — the pen owns the stage until this closes.
#[component]
fn DrawToolbar(video_id: String) -> Element {
    let mut draw = DrawCtx::use_ctx();
    let selected = (draw.color)();

    rsx! {
        div { class: "flex h-12 shrink-0 items-center gap-2 px-4 bg-card/80 border-t border-purple-500/30",
            span { class: "text-[11px] uppercase tracking-wider text-purple-400 font-medium mr-1",
                "Drawing"
            }
            for color in super::STROKE_COLORS {
                button {
                    class: if selected == color { "h-5 w-5 rounded-full ring-2 ring-indigo-400 ring-offset-1 ring-offset-card" } else { "h-5 w-5 rounded-full hover:scale-110 transition-transform" },
                    style: "background:{color}",
                    title: "Stroke color",
                    onclick: move |_| draw.color.set(color),
                }
            }
            div { class: "mx-1 h-4 w-px bg-border" }
            button {
                class: BTN,
                title: "Undo last stroke",
                onclick: move |_| {
                    draw.pending.write().pop();
                },
                Undo2 { size: 16 }
            }
            button {
                class: "flex h-7 items-center rounded-md px-2 text-xs text-muted-foreground hover:text-foreground hover:bg-muted/50",
                onclick: move |_| {
                    draw.pending.set(Vec::new());
                    draw.pending_at.set(None);
                },
                "Clear"
            }
            div { class: "flex-1" }
            span { class: "text-[11px] text-muted-foreground hidden sm:block",
                "The drawing posts with your next comment, pinned to this frame."
            }
            button {
                class: BTN,
                title: "Cancel drawing",
                onclick: {
                    let _video_id = video_id;
                    // Discard, don't carry: strokes left armed after
                    // exit would post pinned to whatever frame the
                    // user scrubbed to next.
                    move |_| draw.cancel_drawing()
                },
                X { size: 16 }
            }
        }
    }
}
