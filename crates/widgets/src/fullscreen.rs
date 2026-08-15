//! **Experiences** — the fullscreen chrome a note widget's immersive
//! view (and the shell's own full-screen flows, e.g. inbox triage)
//! renders inside.
//!
//! [`FullscreenExperience`] is the chrome: a fixed, viewport-covering
//! overlay (so it escapes the pane/sidebar layout regardless of nesting)
//! with a slim top bar and an `Esc`-to-exit affordance. The experience's
//! own content — the setlist player today, more to come — renders inside.
//!
//! Which note opens which experience is the widget registry's business
//! (`type:` / `experience:` frontmatter claims — see the crate docs): a
//! widget whose spec sets `fullscreen_owns_body` mounts this chrome
//! while its [`WidgetCtx::fullscreen`](crate::WidgetCtx) signal is set.

use architect_ui::lucide_dioxus::Minimize2;
use dioxus::prelude::*;

/// Full-screen Experience chrome: a viewport overlay with a top bar
/// (title + exit) that renders `children` beneath. `on_exit` fires on
/// `Esc` or the close control.
#[component]
pub fn FullscreenExperience(
    title: String,
    on_exit: EventHandler<()>,
    /// Handle `Esc` at the chrome level. Set false when the inner content
    /// already binds `Esc` (e.g. the inbox deck) so exit fires once.
    #[props(default = true)]
    handle_esc: bool,
    children: Element,
) -> Element {
    // Double-Esc to exit: the experience can host a vim editor whose own Esc
    // leaves insert mode — a single Esc must NOT drop the whole experience.
    // The first Esc arms; a second within the window exits (the arm auto-clears
    // so a stray Esc leaving insert mode never accumulates into an exit).
    let mut esc_armed = use_signal(|| false);
    rsx! {
        div {
            // `fixed inset-0` escapes the pane/sidebar layout and covers the
            // whole viewport. `tabindex` + autofocus so the Esc keydown lands
            // here immediately (keydown bubbles up from focused children).
            class: "fixed inset-0 z-50 flex flex-col bg-background outline-none",
            tabindex: "0",
            autofocus: true,
            onkeydown: move |e| {
                if handle_esc && e.key() == Key::Escape {
                    if *esc_armed.peek() {
                        e.prevent_default();
                        esc_armed.set(false);
                        on_exit.call(());
                    } else {
                        esc_armed.set(true);
                        spawn(async move {
                            #[cfg(target_arch = "wasm32")]
                            gloo_timers::future::TimeoutFuture::new(900).await;
                            esc_armed.set(false);
                        });
                    }
                }
            },
            // Slim top bar: title on the left, Esc-to-exit on the right.
            div { class: "flex shrink-0 items-center justify-between border-b border-border px-4 py-2",
                span { class: "truncate text-sm font-semibold text-foreground", "{title}" }
                button {
                    class: "flex items-center gap-1.5 rounded-md border border-border px-2 py-1 text-xs text-muted-foreground hover:bg-accent hover:text-foreground",
                    title: "Press Esc twice to exit",
                    onclick: move |_| on_exit.call(()),
                    Minimize2 { class: "size-3.5" }
                    span { class: "font-medium",
                        if esc_armed() { "Esc again to exit" } else { "Esc Esc" }
                    }
                }
            }
            // Experience content fills the rest of the viewport.
            div { class: "flex min-h-0 flex-1 flex-col overflow-hidden", {children} }
        }
    }
}

/// Compact control to (re-)enter the full-screen experience from the
/// embedded fallback view.
#[component]
pub fn EnterExperienceButton(label: String, on_enter: EventHandler<()>) -> Element {
    rsx! {
        div { class: "flex items-center justify-between border-b border-border/60 px-4 py-2",
            span { class: "text-xs font-medium uppercase tracking-wide text-muted-foreground", "{label}" }
            button {
                class: "rounded-md bg-primary px-2.5 py-1 text-xs font-semibold text-primary-foreground hover:bg-primary/90",
                onclick: move |_| on_enter.call(()),
                "Open full-screen"
            }
        }
    }
}
