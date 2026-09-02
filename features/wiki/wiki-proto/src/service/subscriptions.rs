//! Managing what a vault or wiki subscribes to.
//!
//! Every method names its subscriber, because subscriptions are held
//! per vault and per wiki rather than per org — declining scripture in
//! one wiki must not decline it in the org's vault
//! (`wiki.core.optional`).

use crate::error::WikiError;
use crate::subscription::{Subscriber, Subscription};

/// One subscription plus what the server knows about its local copy.
///
/// The staleness half is not decoration: `wiki.subscribe.local-copy`
/// requires a copy to record which upstream state it reflects, and a
/// person deciding whether to refresh has no other way to tell.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[repr(C)]
pub struct HeldSubscription {
    pub subscription: Subscription,
    /// Files in the local copy. Zero means it has never been
    /// materialized.
    pub files: u32,
    /// Pages the subscriber has changed or added that upstream has not
    /// seen. Unsubscribing with these outstanding must ask first.
    pub local_changes: u32,
    /// Pages where both sides changed, awaiting a person.
    pub conflicts: u32,
}

/// What a refresh did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[repr(C)]
pub struct RefreshReport {
    /// The source refreshed, as `domain/slug`.
    pub qualified: String,
    pub pulled: u32,
    pub in_sync: u32,
    /// Paths held locally that upstream does not have.
    pub local_only: Vec<String>,
    /// Paths both sides changed. Never resolved by the refresh.
    pub conflicted: Vec<String>,
}

#[architect::rpc]
pub trait Subscriptions {
    /// Everything a subscriber holds, declined entries included — a
    /// declined core subscription has to be visible or turning it back
    /// on is impossible.
    fn list_subscriptions(
        &self,
        subscriber: Subscriber,
    ) -> Result<Vec<HeldSubscription>, WikiError>;

    /// Take on a source.
    ///
    /// Re-subscribing to a declined source clears the decline; to one
    /// already active is an error, because the two look identical
    /// afterwards and only one was meant.
    fn subscribe(
        &self,
        subscriber: Subscriber,
        subscription: Subscription,
    ) -> Result<(), WikiError>;

    /// Drop a source, by its `domain/slug`.
    ///
    /// A core subscription is declined rather than removed, so a later
    /// sweep re-offers it instead of silently restoring it. `force`
    /// answers the question a copy with unpushed local work asks:
    /// without it, such a copy refuses rather than discarding the
    /// work (`wiki.subscribe.local-copy`).
    fn unsubscribe(
        &self,
        subscriber: Subscriber,
        qualified: &str,
        force: bool,
    ) -> Result<(), WikiError>;

    /// Bring one subscribed source's local copy up to date.
    fn refresh_subscription(
        &self,
        subscriber: Subscriber,
        qualified: &str,
    ) -> Result<RefreshReport, WikiError>;

    /// What this deployment subscribes everyone to
    /// (`wiki.core.default`). Read-only: core membership is a property
    /// of the deployment, and a client that could edit it would be
    /// changing what every other vault gets.
    fn core_set(&self) -> Result<Vec<Subscription>, WikiError>;

    /// Sources open to subscription that this server can see: every
    /// public wiki on this data root, other orgs' included
    /// (`wiki.access.visibility`, `wiki.access.directory`). Unlisted
    /// and private wikis are absent — subscribing to an unlisted one
    /// takes its reference, and to a private one is refused.
    fn discover(&self) -> Result<Vec<Subscription>, WikiError>;
}
