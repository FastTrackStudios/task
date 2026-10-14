//! The phone's three gestures, on every screen (Obsidian's):
//!
//! - **Swipe right**: the left drawer — the screen's companion column
//!   (the work sidebar, a project's, the vault's files, a wiki's pages),
//!   or what the page puts there instead.
//! - **Swipe left**: the right drawer — what the screen is about: a
//!   note's properties and links, an issue's properties, the docked
//!   app panels elsewhere.
//! - **Pull down** at the top of a screen: the command palette.
//!
//! Swipe an open drawer back towards its edge, or tap the dimmed
//! screen, to close it; moving to another screen closes it too.
//!
//! **Who renders what.** The shell recognises the strokes and holds the
//! state ([`PhonePanels`]); the drawers themselves are [`PhoneDrawer`]s
//! rendered by whoever owns their content. A page that fills a side
//! renders its own `PhoneDrawer` inside its own tree, so the content's
//! signals are read under the scope that owns them; while it is
//! mounted it *claims* the side and the shell's fallback for that side
//! steps aside.
//!
//! **Recognising.** One document-level script listens to touches
//! (passive — scrolling is never blocked) and reports strokes; Rust
//! decides what they do. Touch events rather than pointer events: in
//! iOS's web view a vertical stroke is claimed by native scrolling,
//! which cancels pointer events mid-gesture. A stroke is classified once
//! it has travelled [`DECIDE`] px — clearly sideways, or clearly down
//! from the top of the page — and left alone otherwise. It is never
//! taken from something that needs the drag itself: a horizontally
//! scrolling strip, a slider, a dialog, anything marked
//! `data-no-swipe`, or a long press (text selection). At `md:` and up
//! nothing listens: the drawers are sidebars there and the palette a
//! keystroke.

use dioxus::prelude::*;

/// Sideways travel that opens (or closes) a drawer on release, in px.
const SWIPE: f64 = 70.0;
/// Downward travel that opens the palette on release.
const PULL: f64 = 90.0;
/// Travel before a stroke is classified (the script's threshold).
#[cfg(test)]
const DECIDE: f64 = 12.0;

/// A drawer's edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
}

/// The stroke under the finger right now.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
enum Live {
    #[default]
    None,
    Sideways(f64),
    Pull(f64),
}

/// The shell's drawer state, provided once by the app shell.
#[derive(Clone, Copy)]
pub struct PhonePanels {
    pub left_open: Signal<bool>,
    pub right_open: Signal<bool>,
    /// Pages' [`PhoneDrawer`]s mounted per side.
    left_claims: Signal<u32>,
    right_claims: Signal<u32>,
    /// Whether the shell has something for a side nobody claims.
    left_fallback: Signal<bool>,
    right_fallback: Signal<bool>,
    live: Signal<Live>,
    /// Whether the screen is phone-sized (below `md:`) — drawers only
    /// mount there; at `md:` and up their content is a sidebar already.
    pub phone: Signal<bool>,
}

impl PhonePanels {
    fn open(self, side: Side) -> Signal<bool> {
        match side {
            Side::Left => self.left_open,
            Side::Right => self.right_open,
        }
    }

    fn claims(self, side: Side) -> Signal<u32> {
        match side {
            Side::Left => self.left_claims,
            Side::Right => self.right_claims,
        }
    }

    fn has(self, side: Side) -> bool {
        let fallback = match side {
            Side::Left => self.left_fallback,
            Side::Right => self.right_fallback,
        };
        *self.claims(side).peek() > 0 || *fallback.peek()
    }

    /// Open `side`'s drawer, if there is one; whether it opened.
    pub fn show(self, side: Side) -> bool {
        if !*self.phone.peek() || !self.has(side) {
            return false;
        }
        self.close();
        let mut open = self.open(side);
        open.set(true);
        true
    }

    /// Close both drawers.
    pub fn close(mut self) {
        if *self.left_open.peek() {
            self.left_open.set(false);
        }
        if *self.right_open.peek() {
            self.right_open.set(false);
        }
    }

    /// The drawer a live sideways stroke is moving, if any.
    fn moving(self) -> Option<(Side, f64)> {
        let Live::Sideways(dx) = (self.live)() else {
            return None;
        };
        side_for(
            dx,
            (self.left_open)(),
            (self.right_open)(),
            self.has(Side::Left),
            self.has(Side::Right),
        )
        .map(|s| (s, dx))
    }
}

/// Provide [`PhonePanels`]. Call once, from the app shell.
pub fn provide_phone_panels() -> PhonePanels {
    use_context_provider(|| PhonePanels {
        left_open: Signal::new(false),
        right_open: Signal::new(false),
        left_claims: Signal::new(0),
        right_claims: Signal::new(0),
        left_fallback: Signal::new(false),
        right_fallback: Signal::new(false),
        live: Signal::new(Live::None),
        phone: Signal::new(false),
    })
}

/// The shell's drawer state.
pub fn use_phone_panels() -> PhonePanels {
    use_context::<PhonePanels>()
}

/// Which drawer a sideways stroke of `dx` drives: an open drawer is
/// closed by a stroke towards its own edge (and only that way); with
/// neither open, rightwards brings the left drawer and leftwards the
/// right one — when there is one.
fn side_for(
    dx: f64,
    left_open: bool,
    right_open: bool,
    has_left: bool,
    has_right: bool,
) -> Option<Side> {
    match (left_open, right_open) {
        (true, _) => (dx < 0.0).then_some(Side::Left),
        (_, true) => (dx > 0.0).then_some(Side::Right),
        _ if dx > 0.0 => has_left.then_some(Side::Left),
        _ => has_right.then_some(Side::Right),
    }
}

/// The document-level recogniser. Reports `["side", dx]`, `["pull",
/// dy]` and `["end"]`; classification and exclusions live here so a
/// stroke never round-trips before the page can scroll.
const GESTURES_JS: &str = r#"
const DECIDE = 12, LONG_PRESS = 350;
let s = null;
const scrollsX = (el) => {
  for (; el && el !== document.body; el = el.parentElement) {
    const o = getComputedStyle(el).overflowX;
    if ((o === 'auto' || o === 'scroll') && el.scrollWidth > el.clientWidth + 1) return true;
  }
  return false;
};
const held = (t) => !!(t.closest && t.closest('[data-no-swipe], input[type=range], [role=dialog], [aria-modal=true]'));
document.addEventListener('touchstart', (e) => {
  if (window.innerWidth >= 768 || e.touches.length !== 1) { s = null; return; }
  const p = e.touches[0];
  s = { x: p.clientX, y: p.clientY, at: Date.now(), top: window.scrollY <= 2,
        target: e.target, kind: null };
}, { passive: true });
document.addEventListener('touchmove', (e) => {
  if (!s || e.touches.length !== 1) return;
  const p = e.touches[0];
  const dx = p.clientX - s.x, dy = p.clientY - s.y;
  if (s.kind === null && Math.hypot(dx, dy) >= DECIDE) {
    const across = Math.abs(dx) > Math.abs(dy) * 1.5;
    const down = dy > 0 && dy > Math.abs(dx) * 1.5;
    const late = Date.now() - s.at > LONG_PRESS;
    if (late || held(s.target)) s.kind = 'none';
    else if (across && !scrollsX(s.target)) s.kind = 'side';
    else if (down && s.top) s.kind = 'pull';
    else s.kind = 'none';
  }
  if (s.kind === 'side') dioxus.send(['side', dx]);
  else if (s.kind === 'pull') dioxus.send(['pull', dy]);
}, { passive: true });
const end = () => {
  if (s && (s.kind === 'side' || s.kind === 'pull')) dioxus.send(['end', 0]);
  s = null;
};
document.addEventListener('touchend', end, { passive: true });
document.addEventListener('touchcancel', end, { passive: true });
await new Promise(() => {});
"#;

/// On a touch screen a note's body takes focus only from a touch inside
/// it, or when it says it may (`data-autofocus="true"`, a fresh empty
/// note). An editor places its caret as it mounts and again as content
/// arrives, and on iOS a caret in a contenteditable is focus: the
/// keyboard rose over every note opened only to read it, and its text
/// gestures then swallowed the swipe meant for a drawer.
const FOCUS_RULE_JS: &str = r#"
if (!window.__taskFocusRule) {
  window.__taskFocusRule = true;
  let touchedAt = 0;
  const coarse = () => window.matchMedia('(pointer: coarse)').matches;
  document.addEventListener('touchstart', (e) => {
    if (e.target.closest && e.target.closest('.note-body')) touchedAt = Date.now();
  }, { passive: true, capture: true });
  // A press on a keyboard toolbar keeps the editor's focus (and the
  // keyboard): the mouse-down that would move focus is cancelled; the
  // click still runs.
  document.addEventListener('mousedown', (e) => {
    if (e.target.closest && e.target.closest('[data-keep-focus]')) e.preventDefault();
  }, true);
  // `editing-doc` on <html> while a document body (a note's, a task's)
  // holds focus — what its keyboard toolbar shows on. Read from the
  // focused element itself, so a blur nobody reported cannot leave it on.
  const mark = () => {
    const a = document.activeElement;
    const on = !!(a && a.closest && a.closest('.note-body, .doc-body'));
    document.documentElement.classList.toggle('editing-doc', on);
  };
  document.addEventListener('focusin', () => setTimeout(mark, 0), true);
  document.addEventListener('focusout', () => setTimeout(mark, 0), true);
  document.addEventListener('focusin', (e) => {
    if (!coarse()) return;
    const body = e.target.closest && e.target.closest('.note-body');
    if (!body || body.dataset.autofocus === 'true') return;
    if (Date.now() - touchedAt < 1500) return;
    e.target.blur();
  }, true);
}
"#;

/// The recogniser, the scrim and the pull cue. Mount once, in the shell.
/// `has_left` / `has_right`: whether the shell renders a fallback
/// drawer for that side.
#[component]
pub fn PhoneGestures(has_left: bool, has_right: bool) -> Element {
    let panels = use_phone_panels();
    let mut palette = crate::palette::use_palette_open();
    {
        let (mut l, mut r) = (panels.left_fallback, panels.right_fallback);
        if *l.peek() != has_left {
            l.set(has_left);
        }
        if *r.peek() != has_right {
            r.set(has_right);
        }
    }
    use_hook(|| {
        let _ = document::eval(FOCUS_RULE_JS);
    });
    use_hook(move || {
        let mut phone = panels.phone;
        spawn(async move {
            let mut ev = document::eval(
                "const q = window.matchMedia('(max-width: 767px)');\
                 q.addEventListener('change', () => dioxus.send(q.matches));\
                 dioxus.send(q.matches);\
                 await new Promise(() => {});",
            );
            while let Ok(now) = ev.recv::<bool>().await {
                if *phone.peek() != now {
                    phone.set(now);
                }
                if !now {
                    panels.close();
                }
            }
        });
    });
    use_hook(move || {
        let mut live = panels.live;
        spawn(async move {
            let mut ev = document::eval(GESTURES_JS);
            while let Ok((kind, d)) = ev.recv::<(String, f64)>().await {
                match kind.as_str() {
                    "side" => live.set(Live::Sideways(d)),
                    "pull" => live.set(Live::Pull(d)),
                    _ => {
                        match *live.peek() {
                            Live::Sideways(dx) if dx.abs() > SWIPE => {
                                let (l, r) = (*panels.left_open.peek(), *panels.right_open.peek());
                                let side = side_for(
                                    dx,
                                    l,
                                    r,
                                    panels.has(Side::Left),
                                    panels.has(Side::Right),
                                );
                                if let Some(side) = side {
                                    let mut open = panels.open(side);
                                    let now = *open.peek();
                                    open.set(!now);
                                }
                            }
                            Live::Pull(dy) if dy > PULL => {
                                panels.close();
                                palette.set(true);
                            }
                            _ => {}
                        }
                        live.set(Live::None);
                    }
                }
            }
        });
    });

    let shown = (panels.left_open)() || (panels.right_open)() || panels.moving().is_some();
    let pull = match (panels.live)() {
        Live::Pull(dy) if dy > 0.0 => Some(dy),
        _ => None,
    };
    rsx! {
        if shown {
            div {
                class: "fixed inset-0 z-40 bg-black/50 md:hidden",
                onclick: move |_| panels.close(),
            }
        }
        if let Some(dy) = pull {
            div {
                class: "pointer-events-none fixed inset-x-0 top-0 z-40 flex justify-center md:hidden",
                style: "transform: translateY(calc(env(safe-area-inset-top, 0px) + {(dy * 0.5).min(70.0)}px)); opacity: {(dy / PULL).min(1.0)}",
                span {
                    class: if dy > PULL {
                        "rounded-full bg-primary px-4 py-1.5 text-sm font-medium text-primary-foreground shadow"
                    } else {
                        "rounded-full bg-muted px-4 py-1.5 text-sm text-muted-foreground shadow"
                    },
                    if dy > PULL { "Release for commands" } else { "Pull for commands" }
                }
            }
        }
    }
}

const DRAWER: &str = "fixed inset-y-0 z-50 flex w-[85vw] max-w-sm flex-col overflow-y-auto bg-background pt-[env(safe-area-inset-top,0px)] pb-[env(safe-area-inset-bottom,0px)] shadow-xl md:hidden";

/// One drawer, following the finger while a stroke drives it. A page
/// renders it inside its own tree to fill `side` while mounted; the
/// shell renders its fallbacks with `fallback: true`, which give way to
/// any page's.
#[component]
pub fn PhoneDrawer(
    side: Side,
    #[props(default)] fallback: bool,
    #[props(default)] testid: Option<String>,
    children: Element,
) -> Element {
    let panels = use_phone_panels();
    use_hook(move || {
        if !fallback {
            let mut c = panels.claims(side);
            *c.write() += 1;
        }
    });
    use_drop(move || {
        if !fallback {
            let mut c = panels.claims(side);
            let n = *c.peek();
            c.set(n.saturating_sub(1));
        }
    });
    if !(panels.phone)() || (fallback && (panels.claims(side))() > 0) {
        return rsx! {};
    }

    let open = (panels.open(side))();
    let dragging = panels
        .moving()
        .filter(|(s, _)| *s == side)
        .map(|(_, dx)| dx);
    let transform = match (side, open, dragging) {
        (Side::Left, true, Some(dx)) => format!("translateX({}px)", dx.min(0.0)),
        (Side::Left, false, Some(dx)) => format!("translateX(calc(-100% + {}px))", dx.max(0.0)),
        (Side::Right, true, Some(dx)) => format!("translateX({}px)", dx.max(0.0)),
        (Side::Right, false, Some(dx)) => format!("translateX(calc(100% + {}px))", dx.min(0.0)),
        (_, true, None) => "translateX(0)".to_owned(),
        (Side::Left, false, None) => "translateX(-100%)".to_owned(),
        (Side::Right, false, None) => "translateX(100%)".to_owned(),
    };
    let edge = match side {
        Side::Left => "left-0 border-r",
        Side::Right => "right-0 border-l",
    };
    let ease = if dragging.is_some() {
        ""
    } else {
        " transition-transform duration-200"
    };
    let testid = testid.unwrap_or_else(|| match side {
        Side::Left => "phone-drawer-left".to_owned(),
        Side::Right => "phone-drawer-right".to_owned(),
    });
    rsx! {
        aside {
            class: "{DRAWER} {edge} border-border{ease}",
            style: "transform: {transform}",
            "aria-hidden": if open { "false" } else { "true" },
            "data-testid": "{testid}",
            {children}
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
        const { assert!(SWIPE > DECIDE * 3.0) };
        const { assert!(PULL > DECIDE * 3.0) };
        assert!(GESTURES_JS.contains(&format!("DECIDE = {DECIDE}")));
    }

    #[test]
    fn right_brings_the_left_drawer_left_the_right_and_back_closes() {
        assert_eq!(side_for(80.0, false, false, true, true), Some(Side::Left));
        assert_eq!(side_for(-80.0, false, false, true, true), Some(Side::Right));
        // Nothing on a side: that way does nothing.
        assert_eq!(side_for(-80.0, false, false, true, false), None);
        assert_eq!(side_for(80.0, false, false, false, true), None);
        // An open drawer closes towards its own edge, and only that way.
        assert_eq!(side_for(-80.0, true, false, true, true), Some(Side::Left));
        assert_eq!(side_for(80.0, true, false, true, true), None);
        assert_eq!(side_for(80.0, false, true, true, true), Some(Side::Right));
        assert_eq!(side_for(-80.0, false, true, true, true), None);
    }
}
