//! What the app shows while it starts: finding the server, then
//! restoring (or making) the session.
//!
//! This is the first thing anyone sees on every launch, and on a phone
//! it follows straight on from the system launch screen (the
//! `LaunchBackground` colour, `apps/mobile/ios/app-plist.sh`). It used
//! to be one line of grey text over the full sign-in form folded into a
//! "Taking too long?" disclosure — the escape hatch shown on a launch
//! that takes a second, which made every start look like a problem.
//!
//! Now it is the app's mark drawing itself in, the name, and one line
//! saying what is happening. The escape hatch is still here — this
//! state must never be a dead end (see `SignInGate`) — but it appears
//! only once the wait is long enough to be worth offering it, or at once
//! when discovery has failed and there is an error to show.

use dioxus::prelude::*;

/// How long a start may take before the way out is offered.
const HELP_AFTER_SECS: u64 = 6;

/// The mark's check, the same path as the app icon (`apps/mobile/ios/icon.svg`),
/// on its 1024-unit grid.
const CHECK_PATH: &str = "M 296 528 L 452 684 L 734 366";

const LOADING_CSS: &str = r"
.task-loading-plate {
  background: linear-gradient(180deg, #232327 0%, #151517 55%, #0a0a0c 100%);
  box-shadow: 0 0 0 1px rgb(255 255 255 / 0.06), 0 18px 48px -12px rgb(108 99 255 / 0.45);
}
.task-loading-check {
  stroke-dasharray: 760;
  stroke-dashoffset: 760;
  animation: task-loading-draw 900ms cubic-bezier(0.65, 0, 0.35, 1) 120ms forwards;
}
.task-loading-glow {
  opacity: 0;
  animation: task-loading-fade 900ms ease-out 500ms forwards;
}
.task-loading-rise {
  opacity: 0;
  transform: translateY(6px);
  animation: task-loading-rise 500ms ease-out 350ms forwards;
}
.task-loading-bar {
  position: relative;
  overflow: hidden;
}
.task-loading-bar::after {
  content: '';
  position: absolute;
  inset: 0;
  width: 40%;
  border-radius: inherit;
  background: linear-gradient(90deg, transparent, #818cf8, transparent);
  animation: task-loading-sweep 1.4s ease-in-out infinite;
}
@keyframes task-loading-draw { to { stroke-dashoffset: 0; } }
@keyframes task-loading-fade { to { opacity: 0.85; } }
@keyframes task-loading-rise { to { opacity: 1; transform: none; } }
@keyframes task-loading-sweep {
  from { transform: translateX(-100%); }
  to { transform: translateX(250%); }
}
@media (prefers-reduced-motion: reduce) {
  .task-loading-check { animation: none; stroke-dashoffset: 0; }
  .task-loading-glow { animation: none; opacity: 0.85; }
  .task-loading-rise { animation: none; opacity: 1; transform: none; }
  .task-loading-bar::after { animation: none; width: 100%; opacity: 0.4; }
}
";

/// The mark at rest — the sign-in page's header, so launch, loading and
/// signing in read as one sequence rather than three screens.
#[component]
pub fn TaskMark() -> Element {
    rsx! {
        div { class: "task-loading-plate flex size-12 items-center justify-center rounded-[14px]",
            style { {LOADING_CSS} }
            svg {
                class: "size-9",
                view_box: "0 0 1024 1024",
                "aria-hidden": "true",
                defs {
                    linearGradient {
                        id: "task-mark-body",
                        gradient_units: "userSpaceOnUse",
                        x1: "180",
                        y1: "200",
                        x2: "860",
                        y2: "880",
                        stop { offset: "0", stop_color: "#CAC5FF" }
                        stop { offset: "0.42", stop_color: "#6C63FF" }
                        stop { offset: "1", stop_color: "#372BC6" }
                    }
                }
                path {
                    d: CHECK_PATH,
                    fill: "none",
                    stroke: "url(#task-mark-body)",
                    stroke_width: "94",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                }
            }
        }
    }
}

/// The start-up screen. `status` is the one line under the name;
/// `error` (discovery's, when it has one) shows the way out at once;
/// `help` is that way out — the sign-in form and server picker.
#[component]
pub fn LoadingScreen(status: String, error: Option<String>, help: Element) -> Element {
    let mut waited = use_signal(|| false);
    use_future(move || async move {
        architect::sleep(architect::Duration::from_secs(HELP_AFTER_SECS)).await;
        waited.set(true);
    });
    let offer_help = waited() || error.is_some();

    rsx! {
        style { {LOADING_CSS} }
        div {
            class: "flex min-h-screen flex-col items-center justify-center gap-8 p-6",
            role: "status",
            aria_live: "polite",
            div { class: "flex flex-col items-center gap-5",
                div { class: "task-loading-plate flex size-20 items-center justify-center rounded-[22px]",
                    svg {
                        class: "size-14",
                        view_box: "0 0 1024 1024",
                        "aria-hidden": "true",
                        defs {
                            linearGradient {
                                id: "task-loading-body",
                                gradient_units: "userSpaceOnUse",
                                x1: "180",
                                y1: "200",
                                x2: "860",
                                y2: "880",
                                stop { offset: "0", stop_color: "#CAC5FF" }
                                stop { offset: "0.42", stop_color: "#6C63FF" }
                                stop { offset: "1", stop_color: "#372BC6" }
                            }
                            filter {
                                id: "task-loading-halo",
                                x: "-45%",
                                y: "-45%",
                                width: "190%",
                                height: "190%",
                                feGaussianBlur { std_deviation: "36" }
                            }
                        }
                        path {
                            class: "task-loading-glow",
                            d: CHECK_PATH,
                            fill: "none",
                            stroke: "#6C63FF",
                            stroke_width: "94",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            filter: "url(#task-loading-halo)",
                        }
                        path {
                            class: "task-loading-check",
                            d: CHECK_PATH,
                            fill: "none",
                            stroke: "url(#task-loading-body)",
                            stroke_width: "94",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                        }
                    }
                }
                div { class: "task-loading-rise flex flex-col items-center gap-3",
                    span { class: "text-xl font-semibold tracking-tight text-foreground", "Task" }
                    div { class: "task-loading-bar h-[3px] w-28 rounded-full bg-muted" }
                    p { class: "text-sm text-muted-foreground", "{status}" }
                }
            }
            if offer_help {
                div { class: "flex w-full max-w-sm flex-col items-center gap-3",
                    if let Some(msg) = error {
                        p { class: "text-center text-xs text-destructive", "{msg}" }
                    }
                    details { class: "w-full text-sm",
                        summary { class: "cursor-pointer py-2 text-center text-muted-foreground",
                            "Taking too long?"
                        }
                        div { class: "flex flex-col gap-4 pt-3", {help} }
                    }
                }
            }
        }
    }
}
