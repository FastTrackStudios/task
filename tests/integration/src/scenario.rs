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
/// Every chapter that moves bytes needs this first. The byte lane reads
/// the checkpoint head rather than the disk — a file being written to
/// right now has no stable length and no stable content — so nothing is
/// readable, streamable or syncable until something has pinned it.
pub async fn pin(server: &Server, root: RootId, why: &str) {
    server
        .backend
        .checkpoint(root, Some(why.to_string()))
        .await
        .expect("checkpoint");
}
