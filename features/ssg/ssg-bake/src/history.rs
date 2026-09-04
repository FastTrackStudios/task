//! A history for a page that has none.
//!
//! The router reads the current URL from a [`History`] in context. In a
//! browser that is the real one; here it is a single fixed route, which
//! is the honest model of a baked page: it is one URL, rendered once,
//! and nothing about it will ever navigate. A visitor who follows a link
//! loads another file.
//!
//! Written out rather than using `dioxus_history::MemoryHistory` so this
//! crate needs no pin on `dioxus-history` beyond what `dioxus` already
//! carries. The behaviour differs anyway: memory history accumulates a
//! stack, and every push here is a no-op.

use dioxus::prelude::History;

/// The one route a baked page is at.
pub(crate) struct BakedHistory {
    route: String,
}

impl BakedHistory {
    pub(crate) fn at(route: impl Into<String>) -> Self {
        Self {
            route: route.into(),
        }
    }
}

impl History for BakedHistory {
    fn current_route(&self) -> String {
        self.route.clone()
    }

    fn can_go_back(&self) -> bool {
        false
    }

    fn can_go_forward(&self) -> bool {
        false
    }

    // Nothing navigates during a render, and a render is all that
    // happens here. If something did try, silently doing nothing is
    // right: the output is the page at `route`, and a push that moved it
    // would mean the file's contents no longer matched its own URL.
    fn go_back(&self) {}

    fn go_forward(&self) {}

    fn push(&self, _route: String) {}

    fn replace(&self, _path: String) {}
}
