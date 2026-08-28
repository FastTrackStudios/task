//! Who is signed in — as a context any feature UI can read.
//!
//! The auth machinery (accounts, tokens, switching) lives in the app
//! crate and feature crates must not depend on it; but "what name do I
//! sign this with" is a question the review composer, comment rails
//! and presence chips all ask. This is that one answer, provided at
//! the app root and consumed with `try_use_context` — a feature
//! surface that mounts without it (the guest lane, a test) simply
//! falls back to asking.

use dioxus::prelude::*;
use uuid::Uuid;

/// The signed-in person, as much as a feature surface needs to know.
#[derive(Clone, Debug, PartialEq)]
pub struct IdentityInfo {
    /// The auth system's user uuid.
    pub user_id: Uuid,
    pub email: String,
    /// Display name — what a comment or presence chip shows.
    pub name: String,
}

/// Copyable context handle. `None` = nobody signed in (or the surface
/// is mounted outside the signed-in app, e.g. a guest share).
#[derive(Clone, Copy)]
pub struct CurrentIdentity(pub Signal<Option<IdentityInfo>>);

impl CurrentIdentity {
    /// The display name to attribute an action to, if anyone is here.
    #[must_use]
    pub fn name(&self) -> Option<String> {
        self.0.read().as_ref().map(|i| i.name.clone())
    }
}
