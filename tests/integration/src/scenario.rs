//! One concept: the assembled world.
//!
//! Every test boots its own. That is deliberate and it costs a second
//! or two per test: sharing one world between tests makes them
//! order-dependent, and an integration suite whose failures move when
//! you run a subset is worse than a slow one. `cargo nextest` runs them
//! in parallel processes anyway.
//!
//! What a test gets is the state at the end of setup, not at the end of
//! the story: two orgs booted, both roots adopted, four people hired.
//! Anything a chapter does to that world it does itself, so reading one
//! test tells you the whole of what it assumes.

use crate::orgs::{Orgs, adopt};
use crate::people::People;
use crate::server::Server;
use files::RootId;
use files::service::version::VersionService;

/// Two companies, their roots, and the four accounts.
pub struct Scenario {
    pub orgs: Orgs,
    /// ACME's session root, on ACME's server.
    pub acme_root: RootId,
    /// VNT's cut root, on VNT's server.
    pub vnt_root: RootId,
    pub people: People,
}

impl Scenario {
    /// A signed-in session as ACME's owner.
    ///
    /// Most chapters want a caller who is allowed to do the thing, so
    /// that what they assert is the behaviour rather than the refusal.
    /// The refusals get their own tests, and they name who is being
    /// refused.
    pub async fn as_alice(&self) -> crate::client::Session {
        crate::client::Session::open(&self.orgs.acme, self.people.alice.token.clone()).await
    }

    /// A signed-in session as VNT's owner, on VNT's server.
    pub async fn as_victor(&self) -> crate::client::Session {
        crate::client::Session::open(&self.orgs.vnt, self.people.victor.token.clone()).await
    }

    /// Boot both servers, adopt both trees, hire everybody.
    pub async fn open() -> Self {
        let orgs = Orgs::boot().await;
        // `files.adopt.in-place`: the trees already exist, written by
        // other applications. Nothing is moved, copied or renamed.
        let acme_root = adopt(&orgs.acme, "Song").await;
        let vnt_root = adopt(&orgs.vnt, "Cut").await;
        let people = People::hire(&orgs, acme_root, vnt_root).await;
        Self {
            orgs,
            acme_root,
            vnt_root,
            people,
        }
    }
}

/// Pin a root's current content, and say why in the message.
///
/// Adoption pins what it read, so a freshly opened scenario needs no
/// call to this — it is here for the chapters that write first and then
/// need the byte lane to see what they wrote, since the lane reads the
/// pinned head rather than the disk.
pub async fn pin(server: &Server, root: RootId, why: &str) {
    server
        .backend
        .checkpoint(root, Some(why.to_string()))
        .await
        .expect("checkpoint");
}
