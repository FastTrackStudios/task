//! Window-chrome hooks for a frameless desktop window.
//!
//! The desktop main runs the window without native decorations and
//! draws its own chrome — drag, minimize/maximize/close — the way the
//! FastTrackStudio app does. The `ui` crate is cross-platform and
//! cannot call `dioxus::desktop`, so the desktop shell provides these
//! callbacks as a context and the shared chrome renders drag surfaces
//! and window controls only when the context is present: web and
//! mobile never provide it, and render exactly what they always did.

use dioxus::prelude::*;

/// What a frameless window needs from its chrome, as callbacks into
/// whatever windowing API the platform main actually has.
#[derive(Clone, Copy, PartialEq)]
pub struct WindowChrome {
    /// Begin a window drag (pointer-down on a title-bar surface).
    pub drag: Callback<()>,
    /// Toggle maximized (double-click on a title-bar surface, or the
    /// middle window button).
    pub toggle_maximize: Callback<()>,
    pub minimize: Callback<()>,
    pub close: Callback<()>,
}

/// The chrome hooks, when a frameless desktop shell provided them.
#[must_use]
pub fn window_chrome() -> Option<WindowChrome> {
    try_consume_context::<WindowChrome>()
}
