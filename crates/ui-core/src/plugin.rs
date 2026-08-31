//! The seam an app plugs into Task through.
//!
//! Task's core — the vault, files, orgs, auth, sync — is the platform.
//! A **plugin app** is a domain on top of it: Cooking, Session,
//! Keyflow, Signal. It brings its own screens and its own vocabulary,
//! and it keeps its data in Task's: markdown notes in the vault, File
//! Roots on disk. That is the trade the whole design turns on — an app
//! gets the file management, sync, sharing and version history for
//! free, and in exchange it stores nothing only it can read.
//!
//! # Why a registry and not a route variant
//!
//! [`crate::nav`] already records why a feature crate cannot name a
//! route: `Route` is one enum in the shell, and a crate outside it
//! cannot add a variant. That module solved the outbound half — a
//! feature linking *out* of itself, via href builders handed down as
//! context.
//!
//! This is the inbound half. The shell keeps one catch-all route,
//! `/app/<id>/<rest>`, and dispatches it here; a plugin supplies a
//! function from its own sub-path to an [`Element`] and never names a
//! route at all. So the routing stays typed where the shell owns it and
//! stringly-typed exactly at the boundary an external crate reaches.
//!
//! # The dependency direction
//!
//! A plugin depends on this crate. This crate depends on no plugin, and
//! the *only* place that names every plugin is the app binary that
//! registers them:
//!
//! ```ignore
//! // apps/desktop/src/main.rs — the composition root, and the one
//! // crate that knows the full list.
//! task_ui_core::plugin::register(cooking_task_plugin::APP);
//! task_ui_core::plugin::register(session_task_plugin::APP);
//! ```
//!
//! Cargo cares about crate cycles, not repo cycles, so a plugin repo
//! that also depends on Task's server API is fine. What must never
//! happen is registration leaking down here or into `task-ui` — the
//! moment this crate names a plugin, the extension point is gone and
//! it is just more coupling with extra steps.
//!
//! # One Dioxus
//!
//! Widgets crossing a crate boundary means every plugin must compile
//! against the byte-identical `dioxus` and `architect-ui`; a skew makes
//! [`Element`] a different type and nothing composes, with an error
//! that never mentions versions. So plugins use [`dioxus`] and
//! [`architect_ui`] *re-exported from here* rather than declaring their
//! own — then there is one version by construction and skew cannot
//! happen.

use dioxus::prelude::*;

/// Dioxus, for plugins to use instead of their own dependency — see the
/// module docs on why that matters.
pub use dioxus;
/// The component library, re-exported for the same reason.
pub use architect_ui;

/// One screen an app contributes to Task's navigation.
#[derive(Clone, Copy)]
pub struct PluginNav {
    /// What the tab says.
    pub label: &'static str,
    /// Its icon. A function rather than an `Element` because an
    /// `Element` cannot be built outside a render.
    pub icon: fn() -> Element,
    /// The app's own path for this screen — `""` is its front page,
    /// `"setlists"` a section within it. Never a Task route: the shell
    /// turns this into `/app/<id>/<path>`.
    pub path: &'static str,
}

/// An app registered into Task.
#[derive(Clone, Copy)]
pub struct PluginApp {
    /// Matches the `task-plugin` catalog id, which is what an org's
    /// manifest turns on and off. An app whose id is not enabled for
    /// the active org contributes nothing — it is not merely hidden,
    /// its screens do not resolve.
    pub id: &'static str,
    /// The screens it puts in the navigation. May be empty: an app can
    /// be reachable only from another app's link, or from a file.
    pub nav: &'static [PluginNav],
    /// Render one of its screens. `path` is whatever followed
    /// `/app/<id>/`, empty for the front page.
    ///
    /// Returning `None` means "not one of mine" and the shell shows its
    /// own not-found rather than the app pretending to have a page.
    pub view: fn(path: &str) -> Option<Element>,
}

/// Everything registered so far.
///
/// A `RwLock` rather than a `OnceLock<Vec<_>>` because registration
/// happens in `main` before the first render but there is no single
/// call that could take the whole list — each plugin registers itself,
/// and a build with different features registers a different set.
static REGISTRY: std::sync::RwLock<Vec<PluginApp>> = std::sync::RwLock::new(Vec::new());

/// Register an app. Call from the composition root, before launch.
///
/// Registering the same id twice replaces the first: a build that
/// somehow lists an app in two places should behave like the one that
/// meant it, not show two identical tabs.
pub fn register(app: PluginApp) {
    let mut registry = REGISTRY.write().expect("plugin registry poisoned");
    match registry.iter_mut().find(|a| a.id == app.id) {
        Some(existing) => *existing = app,
        None => registry.push(app),
    }
}

/// Every registered app, in registration order.
#[must_use]
pub fn registered() -> Vec<PluginApp> {
    REGISTRY
        .read()
        .expect("plugin registry poisoned")
        .clone()
}

/// The app with this id, if one registered.
#[must_use]
pub fn find(id: &str) -> Option<PluginApp> {
    REGISTRY
        .read()
        .expect("plugin registry poisoned")
        .iter()
        .find(|a| a.id == id)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nowhere(_path: &str) -> Option<Element> {
        None
    }

    /// Registering twice under one id replaces rather than duplicates —
    /// two identical tabs is a worse answer than either one.
    #[test]
    fn registering_an_id_twice_keeps_the_last() {
        register(PluginApp {
            id: "test-dup",
            nav: &[],
            view: nowhere,
        });
        register(PluginApp {
            id: "test-dup",
            nav: &[],
            view: nowhere,
        });
        assert_eq!(
            registered().iter().filter(|a| a.id == "test-dup").count(),
            1
        );
    }

    /// An id nobody registered is absent, not empty — the shell needs
    /// to tell "this app is not installed" from "this app has no
    /// screens".
    #[test]
    fn an_unregistered_id_is_absent() {
        assert!(find("test-never-registered").is_none());
    }
}
