//! The phone vault's gestures, Obsidian's:
//!
//! - **Swipe right** on a note: the file drawer slides in from the left,
//!   following the finger, over a dimmed note.
//! - **Swipe left** on a note: the note's own drawer slides in from the
//!   right — its properties, links, graph and share links. Properties
//!   are the document's, not text in the body, so this is where they
//!   are set.
//! - Swipe an open drawer back, tap the dimmed note, or open a file to
//!   close it.
//! - **Pull down** at the top of the note: the command palette (commands,
//!   pages, every note), field at the bottom in thumb reach.
//!
//! Touch events rather than pointer events: in iOS's web view a vertical
//! stroke is claimed by native scrolling, which cancels pointer events
//! mid-gesture, while touch events keep arriving. A stroke is classified
//! once it has travelled [`DECIDE`] points — clearly sideways or clearly
//! down — and anything else is left to scroll as normal. Hidden at `md:`
//! and up, where the explorer and the inspector are sidebars and the
//! palette a keystroke.

use dioxus::prelude::*;

/// Travel before a stroke is classified, in CSS px.
const DECIDE: f64 = 12.0;
/// Sideways travel that opens (or closes) a drawer on release.
const SWIPE: f64 = 70.0;
/// Downward travel that opens the palette on release.
const PULL: f64 = 90.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stroke {
    Undecided,
    Sideways,
    Pull,
    Ignore,
}

/// Which drawer a sideways stroke of `dx` drives, given which is open:
/// an open drawer is closed by a stroke towards its edge; with neither
/// open, rightwards brings the files and leftwards the note's drawer
/// (when the note has one).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Files,
    Note,
}

fn side_for(dx: f64, files_open: bool, note_open: bool, has_note: bool) -> Option<Side> {
    match (files_open, note_open) {
        (true, _) => (dx < 0.0).then_some(Side::Files),
        (_, true) => (dx > 0.0).then_some(Side::Note),
        _ if dx > 0.0 => Some(Side::Files),
        _ if has_note => Some(Side::Note),
        _ => None,
    }
}

/// Whether the page is scrolled to its top — the only place a downward
/// stroke is a pull rather than a scroll. One passive scroll listener.
fn use_at_top() -> Signal<bool> {
    let mut top = use_signal(|| true);
    use_hook(move || {
        spawn(async move {
            let mut ev = document::eval(
                "const s = () => dioxus.send(window.scrollY <= 2);\
                 window.addEventListener('scroll', s, { passive: true });\
                 s();\
                 await new Promise(() => {});",
            );
            while let Ok(at) = ev.recv::<bool>().await {
                if *top.peek() != at {
                    top.set(at);
                }
            }
        });
    });
    top
}

const DRAWER: &str = "fixed inset-y-0 z-50 flex w-[85vw] max-w-sm flex-col overflow-y-auto bg-background pt-[env(safe-area-inset-top,0px)] pb-[env(safe-area-inset-bottom,0px)] shadow-xl md:hidden";

/// The note area with its gestures, the file drawer holding `drawer`
/// and, when given, the note's drawer holding `note`.
#[component]
pub fn PhoneGestures(
    open: Signal<bool>,
    drawer: Element,
    note_open: Signal<bool>,
    #[props(default)] note: Option<Element>,
    children: Element,
) -> Element {
    let mut palette = crate::palette::use_palette_open();
    let at_top = use_at_top();
    let mut start = use_signal(|| None::<(f64, f64, bool)>);
    let mut stroke = use_signal(|| Stroke::Undecided);
    let mut delta = use_signal(|| (0.0_f64, 0.0_f64));
    let has_note = note.is_some();

    let mut finish = move || {
        let (dx, dy) = *delta.peek();
        match *stroke.peek() {
            Stroke::Sideways if dx.abs() > SWIPE => {
                let (f, n) = (*open.peek(), *note_open.peek());
                match side_for(dx, f, n, has_note) {
                    Some(Side::Files) => open.set(!f),
                    Some(Side::Note) => note_open.set(!n),
                    None => {}
                }
            }
            Stroke::Pull if dy > PULL => palette.set(true),
            _ => {}
        }
        start.set(None);
        stroke.set(Stroke::Undecided);
        delta.set((0.0, 0.0));
    };

    let sideways = stroke() == Stroke::Sideways;
    let (dx, dy) = delta();
    let live = if sideways {
        side_for(dx, open(), note_open(), has_note)
    } else {
        None
    };
    // Each drawer's offset: 0 open, ∓100% closed, following the finger
    // while a sideways stroke drives it.
    let files_transform = match (open(), live == Some(Side::Files)) {
        (true, true) => format!("translateX({}px)", dx.min(0.0)),
        (false, true) => format!("translateX(calc(-100% + {}px))", dx.max(0.0)),
        (true, false) => "translateX(0)".to_owned(),
        (false, false) => "translateX(-100%)".to_owned(),
    };
    let note_transform = match (note_open(), live == Some(Side::Note)) {
        (true, true) => format!("translateX({}px)", dx.max(0.0)),
        (false, true) => format!("translateX(calc(100% + {}px))", dx.min(0.0)),
        (true, false) => "translateX(0)".to_owned(),
        (false, false) => "translateX(100%)".to_owned(),
    };
    let shown = open() || note_open() || live.is_some();
    let pulling = stroke() == Stroke::Pull && dy > 0.0;
    let armed = dy > PULL;
    let ease = |dragging: bool| {
        if dragging {
            ""
        } else {
            " transition-transform duration-200"
        }
    };

    rsx! {
        div {
            class: "flex min-h-0 min-w-0 flex-1 flex-col",
            ontouchstart: move |e: Event<TouchData>| {
                let Some(t) = e.data().touches().into_iter().next() else { return };
                let p = t.client_coordinates();
                start.set(Some((p.x, p.y, *at_top.peek())));
                stroke.set(Stroke::Undecided);
                delta.set((0.0, 0.0));
            },
            ontouchmove: move |e: Event<TouchData>| {
                let Some((x0, y0, top)) = *start.peek() else { return };
                let Some(t) = e.data().touches().into_iter().next() else { return };
                let p = t.client_coordinates();
                let (dx, dy) = (p.x - x0, p.y - y0);
                if *stroke.peek() == Stroke::Undecided && dx.hypot(dy) >= DECIDE {
                    let across = dx.abs() > dy.abs() * 1.5;
                    let down = dy > 0.0 && dy > dx.abs() * 1.5;
                    stroke.set(if across {
                        Stroke::Sideways
                    } else if down && top {
                        Stroke::Pull
                    } else {
                        Stroke::Ignore
                    });
                }
                if matches!(*stroke.peek(), Stroke::Sideways | Stroke::Pull) {
                    delta.set((dx, dy));
                }
            },
            ontouchend: move |_| finish(),
            ontouchcancel: move |_| finish(),
            {children}
        // The pull's cue, following the finger down from the top.
        if pulling {
            div {
                class: "pointer-events-none fixed inset-x-0 top-0 z-40 flex justify-center md:hidden",
                style: "transform: translateY(calc(env(safe-area-inset-top, 0px) + {(dy * 0.5).min(70.0)}px)); opacity: {(dy / PULL).min(1.0)}",
                span {
                    class: if armed {
                        "rounded-full bg-primary px-4 py-1.5 text-sm font-medium text-primary-foreground shadow"
                    } else {
                        "rounded-full bg-muted px-4 py-1.5 text-sm text-muted-foreground shadow"
                    },
                    if armed { "Release for commands" } else { "Pull for commands" }
                }
            }
        }
        // ── the drawers ──
        if shown {
            div {
                class: "fixed inset-0 z-40 bg-black/50 md:hidden",
                onclick: move |_| {
                    open.set(false);
                    note_open.set(false);
                },
            }
        }
        aside {
            class: "{DRAWER} left-0 border-r border-border{ease(live == Some(Side::Files))}",
            style: "transform: {files_transform}",
            "aria-hidden": if open() { "false" } else { "true" },
            "data-testid": "vault-drawer",
            {drawer}
        }
        if let Some(note) = note {
            aside {
                class: "{DRAWER} right-0 border-l border-border{ease(live == Some(Side::Note))}",
                style: "transform: {note_transform}",
                "aria-hidden": if note_open() { "false" } else { "true" },
                "data-testid": "note-drawer",
                {note}
            }
        }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The thresholds leave room for a scroll and a tap: a stroke has to
    // travel well past the classification distance before it acts.
    #[test]
    fn a_gesture_needs_more_than_the_classifying_travel() {
        assert!(SWIPE > DECIDE * 3.0);
        assert!(PULL > DECIDE * 3.0);
    }

    #[test]
    fn right_brings_the_files_left_brings_the_note_and_back_closes() {
        assert_eq!(side_for(80.0, false, false, true), Some(Side::Files));
        assert_eq!(side_for(-80.0, false, false, true), Some(Side::Note));
        // No note open: leftwards does nothing.
        assert_eq!(side_for(-80.0, false, false, false), None);
        // An open drawer closes towards its own edge, and only that way.
        assert_eq!(side_for(-80.0, true, false, true), Some(Side::Files));
        assert_eq!(side_for(80.0, true, false, true), None);
        assert_eq!(side_for(80.0, false, true, true), Some(Side::Note));
        assert_eq!(side_for(-80.0, false, true, true), None);
    }
}
