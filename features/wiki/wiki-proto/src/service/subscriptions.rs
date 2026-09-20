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

    /// **Publisher side.** Every file of a source this org publishes,
    /// with its hash — what a subscriber on another server needs before
    /// it can work out which files it is missing.
    ///
    /// # Why the publisher's own lane, and not a new one
    ///
    /// `LocalOrgs` resolves a source by reading the publisher's
    /// directory, which works precisely because both orgs are on one
    /// data root. Across servers there is no such directory, and the
    /// gap was never the copying — [`materialize::refresh`] has always
    /// been generic over its source — it was that nothing on the far
    /// side would answer "what have you got".
    ///
    /// This answers it, on the service that already owns the question.
    /// `discover` is the same shape one step earlier: it already tells a
    /// stranger which sources exist and already honours visibility in
    /// doing so, so the lane, the mount and the permit table are the
    /// ones that were going to gate this anyway.
    ///
    /// # What decides access
    ///
    /// Two things, and both must hold.
    ///
    /// **`secret` authenticates the caller.** The publisher minted it
    /// for one source with [`Self::grant_source_read`] and can revoke it
    /// with [`Self::revoke_source_read`]; holding it is the whole of the
    /// subscriber's authority, exactly as an `Offer`'s secret is on the
    /// files lane. This matters more than it looks: these two methods
    /// are reachable without a session, because the caller is another
    /// server rather than a person and has no role here — and the rule
    /// for that surface is that a service on it authenticates its own
    /// callers. Visibility alone would not; it authorises the
    /// *resource* and would let anyone ask.
    ///
    /// **Visibility authorises the source** (`wiki.access.visibility`),
    /// checked by the same `admits` that decides it for a subscriber on
    /// the publisher's own data root. A grant does not override it: a
    /// wiki turned private stops answering even to a secret already
    /// issued, which is what makes narrowing take effect on copies
    /// already held.
    ///
    /// # Errors
    ///
    /// [`WikiError`] when the secret grants no such source, when the
    /// source does not exist here, or when its visibility refuses.
    fn source_manifest(
        &self,
        kind: crate::subscription::SourceKind,
        slug: &str,
        secret: &str,
    ) -> Result<vault_proto::Manifest, WikiError>;

    /// **Publisher side.** One file of a published source.
    ///
    /// Gated exactly as [`Self::source_manifest`] is, and deliberately
    /// one file per call: a refresh fetches only what its `Selection`
    /// admits and what it does not already hold, so the shape that
    /// keeps a shared library usable — names cross, gigabytes do not —
    /// is the shape of the call.
    ///
    /// # Errors
    ///
    /// [`WikiError`] when the secret grants no such source, when the
    /// source or the path is not found, or when visibility refuses.
    fn source_file(
        &self,
        kind: crate::subscription::SourceKind,
        slug: &str,
        path: &str,
        secret: &str,
    ) -> Result<vault_proto::FileBytes, WikiError>;

    /// **Publisher side.** Mint a secret letting another server read one
    /// of this org's sources, and return it.
    ///
    /// A write, and member-gated like every other write here: issuing
    /// read access to somebody else's server is the publisher's
    /// decision, made by a person who is already inside the org.
    /// Carrying the result to the subscriber is a message, not a
    /// protocol — the same stance `FederationService::offer` takes, and
    /// for the same reason: an inert capability can travel by any means
    /// without the lane growing a delivery mechanism.
    ///
    /// Idempotent per source: asking twice returns one secret rather
    /// than accumulating grants nobody can enumerate back to a
    /// subscriber.
    ///
    /// # Errors
    ///
    /// [`WikiError`] when this org publishes no such source.
    fn grant_source_read(
        &self,
        kind: crate::subscription::SourceKind,
        slug: &str,
    ) -> Result<String, WikiError>;

    /// **Publisher side.** Revoke a source read grant.
    ///
    /// Binds on the subscriber's next call, because the secret is
    /// checked on every one — the only ordering a revocation across a
    /// server boundary can honestly promise, stated the same way
    /// `FederationService::withdraw` states it. The copy the subscriber
    /// already holds stays on their disk and goes on resolving
    /// (`wiki.life.orphan`); what ends is the refreshing.
    ///
    /// # Errors
    ///
    /// [`WikiError`] when the grant cannot be written.
    fn revoke_source_read(
        &self,
        kind: crate::subscription::SourceKind,
        slug: &str,
    ) -> Result<(), WikiError>;
}
