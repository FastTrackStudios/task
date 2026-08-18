//! Which servers host which orgs — `files.peering.*`.
//!
//! Hosting an org means *knowing* it: its structure, its projects, its
//! catalogue, who its members are. It does not mean holding its content.
//! Those are separate questions and the whole model turns on keeping
//! them separate, because it is what makes a second host cost the size
//! of a catalogue rather than the size of a library.
//!
//! # Every host runs the org
//!
//! Hosting is not caching. A member reaching any host of their org gets
//! the same answers and may do the same things there; a host storing
//! nothing still browses, searches, versions and accepts writes, and
//! fetches content from a host that has it. Which server someone
//! connects to is a question about latency.
//!
//! So nothing here elects a leader or holds a quorum. Structure
//! converges rather than replicating outward from one machine, which is
//! what lets an org run in as many regions as it has users without a
//! control plane to lose. [`Hosting::members`] records where people are
//! homed and never gates what a host will answer.
//!
//! # An org grows by adding hosts
//!
//! There is no limit on how many, and no host has to hold everything: a
//! host holds the share placement put on it, so an org can be larger
//! than any one server hosting it. Were a full copy required, the
//! largest host would cap the org — which is the whole reason
//! structure and content are separable here.
//!
//! That makes [`Content::Complete`] worth asking for by name. A host
//! holding a share is capacity; only a host holding a complete copy is
//! a restore point, and [`Peering::survives_loss_of`] answers on the
//! second, never the first.
//!
//! # No host is the origin
//!
//! The same shape as [`crate::composition`], for the same reason and
//! enforced the same way: a host set has no field for "the one that
//! created it". A server that made an org has no standing the others
//! lack, so losing it costs a copy rather than the org.
//!
//! That is also what makes a backup unremarkable. A host with no members
//! is a backup; it is reached the same way and converges the same way,
//! and restoring is promoting a copy rather than running a recovery
//! tool. Nothing here has a mode for it.
//!
//! # What a peer may see
//!
//! Hosting is granted per org. A peer sees nothing of an org it does not
//! host, and withdrawing one leaves its others untouched — so a shared
//! backup machine is not a way to learn about everyone who uses it.

use std::collections::{BTreeMap, BTreeSet};

/// A server, as a peer rather than an address.
///
/// A public key. The same identity a device is registered against, so
/// "add a backup" and "add a laptop" are the same act with a different
/// grant.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, facet::Facet)]
#[repr(C)]
#[facet(transparent)]
pub struct HostId(pub String);

impl std::fmt::Display for HostId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// An org, by slug.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, facet::Facet)]
#[repr(C)]
#[facet(transparent)]
pub struct OrgId(pub String);

impl std::fmt::Display for OrgId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How much of an org's content a host keeps.
///
/// Not a quantity — placement decides which bytes land where, and this
/// records only whether the answer is "some", "all", or "none". The
/// distinction that matters is [`Self::Complete`]: capacity comes from
/// any host that stores, durability only from one that stores
/// everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, facet::Facet)]
#[repr(u8)]
pub enum Content {
    /// Structure only. Knows the org, stores none of it — the cheap
    /// case, and the one that makes a second host worth having by
    /// default.
    None,
    /// Whatever placement put here. Adds capacity; is not a restore
    /// point, because what it holds is by definition partial.
    Share,
    /// A full copy. What durability is measured against.
    Complete,
}

impl Content {
    /// Whether this host stores anything at all.
    #[must_use]
    pub const fn stores(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// What a host does for one org.
#[derive(Debug, Clone, Copy, PartialEq, Eq, facet::Facet)]
#[repr(C)]
pub struct Hosting {
    /// Whether anyone is homed here.
    ///
    /// Never a capability. Every host serves the whole org — this only
    /// says whether people connect to this one by default, which is
    /// what makes a backup a backup and is deliberately not a variant:
    /// a backup that cannot be read from is not a backup, so it must
    /// behave identically in every other respect.
    pub members: bool,
    /// How much of the org's content this host keeps.
    ///
    /// Structure replicates to every host by definition; content is a
    /// placement decision and this is where its outcome is recorded.
    pub content: Content,
}

impl Hosting {
    /// A host people work on.
    #[must_use]
    pub const fn working() -> Self {
        Self {
            members: true,
            content: Content::Share,
        }
    }

    /// A host that keeps a full copy and serves nobody.
    #[must_use]
    pub const fn backup() -> Self {
        Self {
            members: false,
            content: Content::Complete,
        }
    }

    /// A host that knows the org but holds no bytes — the cheap case,
    /// and the one that makes a second host worth having by default.
    #[must_use]
    pub const fn structure_only() -> Self {
        Self {
            members: false,
            content: Content::None,
        }
    }

    /// A host that stores the share placed on it. How an org grows: its
    /// room is the sum of these.
    #[must_use]
    pub const fn capacity() -> Self {
        Self {
            members: false,
            content: Content::Share,
        }
    }

    /// Whether this hosting is a backup: a full copy, serving nobody.
    #[must_use]
    pub const fn is_backup(self) -> bool {
        matches!(self.content, Content::Complete) && !self.members
    }
}

/// Why a peering change was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerError {
    #[error("{0}: not a host of this org")]
    NotAHost(HostId),
    /// Removing the last host that stores anything would leave the org
    /// with structure and nowhere to read from.
    #[error("{0} is the only host holding content")]
    LastContentHost(HostId),
}

/// Who hosts what.
#[derive(Debug, Clone, Default)]
pub struct Peering {
    hosts: BTreeMap<OrgId, BTreeMap<HostId, Hosting>>,
}

impl Peering {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `host` hosts `org`.
    ///
    /// Idempotent, and re-recording updates what it does — promoting a
    /// structure-only host to hold content is the same call.
    pub fn host(&mut self, org: OrgId, host: HostId, hosting: Hosting) -> &mut Self {
        self.hosts.entry(org).or_default().insert(host, hosting);
        self
    }

    // t[impl files.peering.scope] — revocable per org: this takes an
    // `org` and touches only that org's entry, so withdrawing one leaves
    // the peer's others alone
    /// Stop hosting `org` on `host`.
    ///
    /// Refuses to remove the last host holding content: an org whose
    /// structure survives everywhere and whose bytes survive nowhere is
    /// a worse state than an explicit error, because every path still
    /// resolves and none of them reads.
    pub fn withdraw(&mut self, org: &OrgId, host: &HostId) -> Result<(), PeerError> {
        let Some(hosts) = self.hosts.get_mut(org) else {
            return Err(PeerError::NotAHost(host.clone()));
        };
        let Some(hosting) = hosts.get(host).copied() else {
            return Err(PeerError::NotAHost(host.clone()));
        };
        if hosting.content.stores() && hosts.values().filter(|h| h.content.stores()).count() == 1 {
            return Err(PeerError::LastContentHost(host.clone()));
        }
        hosts.remove(host);
        if hosts.is_empty() {
            self.hosts.remove(org);
        }
        Ok(())
    }

    /// Every host of an org.
    pub fn hosts_of(&self, org: &OrgId) -> impl Iterator<Item = (&HostId, &Hosting)> {
        self.hosts.get(org).into_iter().flatten()
    }

    // t[impl files.peering.scope] — "a peer sees nothing about an org it
    // does not host": the peer's whole world is what this returns, so a
    // shared backup machine is not a way to learn who else uses it
    /// Every org a host serves.
    ///
    /// The peer's whole world: it sees nothing of an org it does not
    /// host, so a shared backup machine is not a way to learn who else
    /// uses it.
    pub fn orgs_on(&self, host: &HostId) -> BTreeSet<&OrgId> {
        self.hosts
            .iter()
            .filter(|(_, hosts)| hosts.contains_key(host))
            .map(|(org, _)| org)
            .collect()
    }

    /// Whether `host` hosts `org` at all.
    #[must_use]
    pub fn hosts_org(&self, host: &HostId, org: &OrgId) -> bool {
        self.hosts
            .get(org)
            .is_some_and(|hosts| hosts.contains_key(host))
    }

    // t[impl files.peering.scale] — "the org's room is the sum of its
    // hosts": the sum is over exactly this set, and nothing here caps how
    // many there may be
    /// Hosts that store any of this org's content — where a read may be
    /// served, and what the org's capacity is the sum of.
    pub fn content_hosts(&self, org: &OrgId) -> impl Iterator<Item = &HostId> {
        self.hosts_of(org)
            .filter(|(_, h)| h.content.stores())
            .map(|(id, _)| id)
    }

    /// Hosts holding a full copy.
    ///
    /// The only hosts an org can be restored from. A host holding a
    /// share is capacity, and counting it here is how a durability
    /// check comes to believe an org is safe when it is not.
    pub fn complete_hosts(&self, org: &OrgId) -> impl Iterator<Item = &HostId> {
        self.hosts_of(org)
            .filter(|(_, h)| matches!(h.content, Content::Complete))
            .map(|(id, _)| id)
    }

    // t[impl files.peering.backup] — a backup is an ordinary peer with no
    // members, not a mode: it is the same `Hosting` shape, filtered, and
    // nothing downstream branches on it
    /// Hosts that keep content and serve nobody.
    pub fn backups(&self, org: &OrgId) -> impl Iterator<Item = &HostId> {
        self.hosts_of(org)
            .filter(|(_, h)| h.is_backup())
            .map(|(id, _)| id)
    }

    /// Whether `host` will answer for `org` — reads, writes, search,
    /// versions, all of it.
    ///
    /// True for every host, including one storing nothing: it fetches
    /// content from a host that has it. The method exists to be the one
    /// place that says so, because the tempting wrong answer — only
    /// hosts holding content can serve — is what turns peers into
    /// caches.
    // t[impl files.peering.serving] — "whether or not that machine stores
    // the bytes involved": true for every host, which is the whole rule.
    // The tempting wrong answer — only content hosts serve — is what
    // turns peers into caches
    #[must_use]
    pub fn serves(&self, host: &HostId, org: &OrgId) -> bool {
        self.hosts_org(host, org)
    }

    /// Whether the org stays available after losing `host`.
    ///
    /// Distinct from [`Self::survives_loss_of`], which asks about the
    /// bytes. Any remaining host keeps serving, because none of them
    /// coordinates the others and there is no quorum to lose.
    #[must_use]
    pub fn available_without(&self, org: &OrgId, host: &HostId) -> bool {
        self.hosts_of(org).any(|(h, _)| h != host)
    }

    /// Whether an org would survive losing `host`.
    ///
    /// Answered on complete copies alone. Two servers each holding half
    /// an org survive nothing — losing either loses whatever only it
    /// had — so a check that counted stores would report an org safe
    /// precisely when scaling it out had made it fragile.
    ///
    /// This is also the reason `files.peering.presence` says no host is
    /// the origin: if this is ever false for a host that merely
    /// happened to create the org, the model has failed.
    #[must_use]
    pub fn survives_loss_of(&self, org: &OrgId, host: &HostId) -> bool {
        self.complete_hosts(org).any(|h| h != host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org(s: &str) -> OrgId {
        OrgId(s.into())
    }
    fn host(s: &str) -> HostId {
        HostId(s.into())
    }

    /// ACME and VNT both hosted on both servers, with content on one.
    fn both() -> Peering {
        let mut p = Peering::new();
        p.host(org("acme"), host("acme-server"), Hosting::working());
        p.host(org("acme"), host("vnt-server"), Hosting::structure_only());
        p.host(org("vnt"), host("vnt-server"), Hosting::working());
        p.host(org("vnt"), host("acme-server"), Hosting::structure_only());
        p
    }

    // t[verify files.peering.presence]
    #[test]
    fn an_org_is_present_on_both_servers_with_content_on_one() {
        let p = both();
        assert_eq!(p.hosts_of(&org("acme")).count(), 2, "both know it");
        assert_eq!(
            p.content_hosts(&org("acme")).count(),
            1,
            "only one holds its bytes"
        );
        assert!(p.hosts_org(&host("vnt-server"), &org("acme")));
    }

    // t[verify files.peering.scope]
    #[test]
    fn a_peer_sees_only_the_orgs_it_hosts() {
        let mut p = both();
        p.host(org("private"), host("acme-server"), Hosting::working());
        assert_eq!(p.orgs_on(&host("vnt-server")).len(), 2);
        assert!(
            !p.orgs_on(&host("vnt-server")).contains(&org("private")),
            "a shared machine is not a way to learn who else uses it"
        );
    }

    // t[verify files.peering.scope]
    #[test]
    fn withdrawing_one_org_leaves_the_others() {
        let mut p = both();
        p.withdraw(&org("acme"), &host("vnt-server")).unwrap();
        assert!(!p.hosts_org(&host("vnt-server"), &org("acme")));
        assert!(
            p.hosts_org(&host("vnt-server"), &org("vnt")),
            "its own org is untouched"
        );
    }

    // t[verify files.peering.backup]
    #[test]
    fn a_backup_is_a_host_with_no_members() {
        let mut p = both();
        p.host(org("acme"), host("offsite"), Hosting::backup());
        assert_eq!(p.backups(&org("acme")).count(), 1);
        // And it is an ordinary content host in every other respect —
        // a backup that cannot be read from is not a backup.
        assert_eq!(p.content_hosts(&org("acme")).count(), 2);
    }

    // t[verify files.peering.presence]
    #[test]
    fn no_host_is_the_origin() {
        let mut p = both();
        // Before a backup, losing the server that made the org loses its
        // content: that is the state peering exists to fix, not a
        // property of who created it.
        assert!(!p.survives_loss_of(&org("acme"), &host("acme-server")));
        p.host(org("acme"), host("offsite"), Hosting::backup());
        assert!(p.survives_loss_of(&org("acme"), &host("acme-server")));
    }

    #[test]
    fn the_last_host_holding_content_cannot_be_withdrawn() {
        let mut p = both();
        // Structure everywhere and bytes nowhere is worse than an error:
        // every path still resolves and none of them reads.
        assert!(matches!(
            p.withdraw(&org("acme"), &host("acme-server")),
            Err(PeerError::LastContentHost(_))
        ));
    }

    // t[verify files.peering.serving]
    #[test]
    fn a_host_storing_nothing_still_serves_the_whole_org() {
        let p = both();
        // vnt-server holds none of ACME's bytes and is still a place an
        // ACME member can work. The tempting wrong answer — only hosts
        // holding content serve — is what turns peers into caches.
        assert_eq!(
            p.hosts_of(&org("acme"))
                .find(|(h, _)| *h == &host("vnt-server"))
                .unwrap()
                .1
                .content,
            Content::None
        );
        assert!(p.serves(&host("vnt-server"), &org("acme")));
    }

    // t[verify files.peering.serving]
    #[test]
    fn losing_a_host_costs_reach_not_availability() {
        let mut p = both();
        for n in 1..=3 {
            p.host(
                org("acme"),
                host(&format!("region-{n}")),
                Hosting::working(),
            );
        }
        // No election, no quorum: whoever is left keeps serving. That is
        // a different question from whether the bytes survived.
        for lost in ["acme-server", "vnt-server", "region-1"] {
            assert!(p.available_without(&org("acme"), &host(lost)));
        }
        assert!(!p.available_without(&org("solo"), &host("nobody")));
    }

    // t[verify files.peering.serving]
    #[test]
    fn a_host_serves_only_the_orgs_it_hosts() {
        let mut p = both();
        p.host(org("private"), host("acme-server"), Hosting::working());
        assert!(!p.serves(&host("vnt-server"), &org("private")));
    }

    // t[verify files.peering.scale]
    #[test]
    fn an_org_grows_by_adding_servers() {
        let mut p = both();
        assert_eq!(p.content_hosts(&org("acme")).count(), 1);
        // Outgrowing a machine is answered by adding a machine.
        for n in 1..=8 {
            p.host(
                org("acme"),
                host(&format!("shelf-{n}")),
                Hosting::capacity(),
            );
        }
        assert_eq!(
            p.content_hosts(&org("acme")).count(),
            9,
            "no limit on how many servers host one org"
        );
    }

    // t[verify files.peering.scale]
    #[test]
    fn an_org_may_be_larger_than_any_server_hosting_it() {
        let mut p = Peering::new();
        // Two half-shares and no full copy: a legitimate state, and the
        // one that makes the split worth having. Were a full copy
        // required of every host, the largest host would cap the org.
        p.host(org("acme"), host("a"), Hosting::capacity());
        p.host(org("acme"), host("b"), Hosting::capacity());
        assert_eq!(p.content_hosts(&org("acme")).count(), 2);
        assert_eq!(p.complete_hosts(&org("acme")).count(), 0);
    }

    // t[verify files.peering.scale]
    #[test]
    fn scaling_out_is_not_durability() {
        let mut p = Peering::new();
        p.host(org("acme"), host("a"), Hosting::capacity());
        p.host(org("acme"), host("b"), Hosting::capacity());
        // Two servers each holding half survive nothing: losing either
        // loses whatever only it had. Counting stores here would report
        // the org safe precisely when scaling out had made it fragile.
        assert!(!p.survives_loss_of(&org("acme"), &host("a")));
        assert!(!p.survives_loss_of(&org("acme"), &host("b")));

        p.host(org("acme"), host("offsite"), Hosting::backup());
        assert!(p.survives_loss_of(&org("acme"), &host("a")));
    }

    #[test]
    fn promoting_a_host_to_hold_content_is_the_same_call() {
        let mut p = both();
        p.host(org("acme"), host("vnt-server"), Hosting::backup());
        assert_eq!(p.content_hosts(&org("acme")).count(), 2);
        assert!(p.survives_loss_of(&org("acme"), &host("acme-server")));
    }
}
