//! Shared foundation for every Task UI crate.
//!
//! `crates/task/ui` (the app shell) used to own all of this privately,
//! which meant a page could only live in the shell: anything extracted
//! into a `features/task/<slice>/<slice>-ui` crate lost `crate::feeds`,
//! `crate::format` and `crate::vox_clients` and had to be rewritten as a
//! props-only dumb component. This crate is the way out — it holds the
//! parts that are genuinely common and carry no product surface:
//!
//! - [`vox_session`] — what base URL do we dial (compile-time on wasm,
//!   env on native) plus the active-server override.
//! - [`vox_clients`] — the generic `establish_for::<C>` / `caller_for`
//!   layer over that URL: one cached connection root per org on wasm,
//!   per-call establish on native. Typed per-service wrappers stay with
//!   whoever owns the service.
//! - [`feeds`] — the `feeds!` declaration macro + the multi-org
//!   fan-out helpers, so a feature crate's RPC calls live with the
//!   feature instead of in the shell.
//! - [`format`] — display formatting (money, playback clocks, status
//!   badges) shared across pages.
//! - [`frontmatter`] — YAML-ish frontmatter reads for vault notes, and
//!   the `type: song` shape the session player consumes.
//! - [`orgs`] — the multi-org selection model every page scopes its
//!   fetches through (discovery itself stays in the shell).
//! - [`states`] — the shared Loading / Error / Empty phase components.
//! - [`nav`] — how a feature page links somewhere the shell owns,
//!   without naming the shell's `Route` enum.
//!
//! It depends on no `*-proto` crate on purpose: adding one here would
//! put it in every consumer's dependency graph, which is exactly the
//! coupling the split is undoing.

pub mod avatar;
pub mod feeds;
pub mod format;
pub mod frontmatter;
pub mod identity;
#[cfg(not(target_arch = "wasm32"))]
pub mod iroh_transport;
pub mod media_grant;
pub mod nav;
pub mod plugin;
pub mod orgs;
pub mod states;
pub mod vox_clients;
pub mod vox_session;
pub mod window_chrome;
