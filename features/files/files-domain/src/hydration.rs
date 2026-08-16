//! What a device holds — `files.sync.selective`, `files.device.control`.
//!
//! This is where [`crate::facet`] stops being a classification and starts
//! deciding something: given a project's facet map and a device's
//! subscription, which paths are resident and which are stubs.
//!
//! Three rules, in precedence order:
//!
//! 1. **A pin wins.** Pinning is the manual override — "I am on a plane
//!    at six" — and it must not be overridable by a facet the device
//!    happens not to subscribe to.
//! 2. **An atomic facet brings its dependencies.** Subscribing to a
//!    session brings the media it references, because a session whose
//!    audio streams in on first access will glitch.
//! 3. **Everything else follows the subscription**, and what is not
//!    subscribed is a stub — present, correctly sized, hydrating on
//!    access. Never absent.
//!
//! Unreachability is deliberately *not* one of these. A location being
//! down is a fact about the network, decided in
//! [`crate::catalogue`]; this module answers what the device *wants*.

use std::collections::BTreeSet;

use files_proto::path::RootPath;
use files_proto::service::tree::Hydration;

use crate::facet::{Facet, FacetMap, Source};

/// What a device asked for from one root.
#[derive(Debug, Clone, Default)]
pub struct Subscription {
    facets: BTreeSet<Facet>,
    pinned: BTreeSet<RootPath>,
    /// Take everything, whatever the facets. What a studio's own NAS
    /// wants, and what nobody's laptop wants.
    everything: bool,
}

impl Subscription {
    #[must_use]
    pub fn new(facets: impl IntoIterator<Item = Facet>) -> Self {
        Self {
            facets: facets.into_iter().collect(),
            pinned: BTreeSet::new(),
            everything: false,
        }
    }

    #[must_use]
    pub fn everything() -> Self {
        Self {
            facets: BTreeSet::new(),
            pinned: BTreeSet::new(),
            everything: true,
        }
    }

    /// Pin a path for offline use regardless of facet. Survives restart,
    /// because it is part of the subscription rather than a cache hint.
    // t[impl files.device.control] — pin survives restart, beats facets
    pub fn pin(&mut self, path: RootPath) -> &mut Self {
        self.pinned.insert(path);
        self
    }

    pub fn unpin(&mut self, path: &RootPath) -> &mut Self {
        self.pinned.remove(path);
        self
    }

    pub fn subscribe(&mut self, facet: Facet) -> &mut Self {
        self.facets.insert(facet);
        self
    }

    pub fn unsubscribe(&mut self, facet: &Facet) -> &mut Self {
        self.facets.remove(facet);
        self
    }

    #[must_use]
    pub fn facets(&self) -> impl Iterator<Item = &Facet> {
        self.facets.iter()
    }

    #[must_use]
    pub fn pinned(&self) -> impl Iterator<Item = &RootPath> {
        self.pinned.iter()
    }

    /// Whether `path` is pinned, directly or by an ancestor. Pinning a
    /// folder pins what is in it — otherwise pinning a session before a
    /// flight would bring the `.rpp` and none of its audio.
    #[must_use]
    pub fn is_pinned(&self, path: &RootPath) -> bool {
        self.pinned.iter().any(|p| path.is_within(p))
    }
}

/// Why a path is resident. Reported rather than collapsed to a boolean,
/// so a UI can explain "this is here because you pinned it" versus
/// "because you take sessions" — and so a user unsubscribing a facet is
/// not surprised that pinned content stays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    Pinned,
    /// Subscribed to this facet directly.
    Facet(Facet),
    /// Pulled in by an atomic facet the device does take.
    Dependency(Facet),
    /// The device takes everything.
    Everything,
    /// Not subscribed, and not pinned.
    NotSubscribed,
}

/// What a device should hold at a path, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub hydration: Hydration,
    pub reason: Reason,
}

impl Decision {
    fn resident(reason: Reason) -> Self {
        Self {
            hydration: Hydration::Resident,
            reason,
        }
    }

    fn stub() -> Self {
        Self {
            hydration: Hydration::Stub,
            reason: Reason::NotSubscribed,
        }
    }
}

/// Decide one path.
///
/// `path` is the file; the facet is resolved from it and from every
/// ancestor, because tool directories classify their whole subtree —
/// `Audio Files/kick.wav` belongs to sessions by virtue of its parent.
#[must_use]
// t[impl files.sync.selective]
pub fn decide(path: &RootPath, facets: &FacetMap, sub: &Subscription) -> Decision {
    if sub.is_pinned(path) {
        return Decision::resident(Reason::Pinned);
    }
    if sub.everything {
        return Decision::resident(Reason::Everything);
    }

    let Some(binding) = classify(path, facets) else {
        // Unmapped: follow the project default if it has one, and
        // otherwise stub. Never guess a facet — that is how footage
        // lands on the wrong tier.
        return match facets.default_facet() {
            Some(d) if sub.facets.contains(d) => Decision::resident(Reason::Facet(d.clone())),
            _ => Decision::stub(),
        };
    };

    let Some(facet) = binding.facet else {
        return Decision::stub();
    };

    if sub.facets.contains(&facet) {
        let reason = if binding.atomic {
            Reason::Dependency(facet)
        } else {
            Reason::Facet(facet)
        };
        return Decision::resident(reason);
    }

    Decision::stub()
}

/// Resolve a path's facet from itself or its nearest classified
/// ancestor.
fn classify(path: &RootPath, facets: &FacetMap) -> Option<crate::facet::Binding> {
    let mut probe = Some(path.clone());
    while let Some(p) = probe {
        let binding = facets.resolve(p.as_str());
        if binding.source != Source::Unmapped {
            return Some(binding);
        }
        probe = p.parent();
    }
    None
}

/// Decide a whole listing at once — what a sync pass works from.
pub fn plan<'a, I>(paths: I, facets: &FacetMap, sub: &Subscription) -> Vec<(&'a RootPath, Decision)>
where
    I: IntoIterator<Item = &'a RootPath>,
{
    paths
        .into_iter()
        .map(|p| {
            let d = decide(p, facets, sub);
            (p, d)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facet::Capability;

    fn p(s: &str) -> RootPath {
        RootPath::parse(s).unwrap()
    }

    fn album() -> FacetMap {
        let mut m = FacetMap::new([Capability::MusicProduction, Capability::VideoProduction]);
        m.map("Project Assembly", Facet::new("assembly"));
        m
    }

    #[test]
    // t[verify files.sync.selective]
    fn a_mix_engineer_takes_sessions_and_leaves_footage() {
        let m = album();
        let sub = Subscription::new([Facet::new("sessions")]);

        let session = decide(&p("Song/Audio Files/kick.wav"), &m, &sub);
        assert_eq!(session.hydration, Hydration::Resident);

        let footage = decide(&p("Proxies/reel.mov"), &m, &sub);
        assert_eq!(footage.hydration, Hydration::Stub);
        assert_eq!(footage.reason, Reason::NotSubscribed);
    }

    #[test]
    fn a_video_editor_takes_the_other_half() {
        let m = album();
        let sub = Subscription::new([Facet::new("proxies")]);
        assert_eq!(
            decide(&p("Proxies/reel.mov"), &m, &sub).hydration,
            Hydration::Resident
        );
        assert_eq!(
            decide(&p("Song/Audio Files/kick.wav"), &m, &sub).hydration,
            Hydration::Stub
        );
    }

    #[test]
    fn a_tool_directory_classifies_its_whole_subtree() {
        let m = album();
        let sub = Subscription::new([Facet::new("sessions")]);
        // Nothing maps this path directly; its parent does.
        let deep = p("01 ALL THAT I AM/Audio Files/Vox/148-V LEAD.wav");
        assert_eq!(decide(&deep, &m, &sub).hydration, Hydration::Resident);
    }

    #[test]
    // t[verify files.sync.selective]
    fn subscribing_to_a_session_brings_its_media() {
        let m = album();
        let sub = Subscription::new([Facet::new("sessions")]);
        let d = decide(&p("Song/Audio Files/kick.wav"), &m, &sub);
        assert_eq!(
            d.reason,
            Reason::Dependency(Facet::new("sessions")),
            "atomic: a session that streams its audio on first play glitches"
        );
    }

    #[test]
    // t[verify files.device.control]
    fn a_pin_beats_an_unsubscribed_facet() {
        let m = album();
        let mut sub = Subscription::new([Facet::new("sessions")]);
        sub.pin(p("Proxies/reel.mov"));

        let d = decide(&p("Proxies/reel.mov"), &m, &sub);
        assert_eq!(d.hydration, Hydration::Resident);
        assert_eq!(d.reason, Reason::Pinned);
    }

    #[test]
    // t[verify files.device.control]
    fn pinning_a_folder_pins_what_is_in_it() {
        let m = album();
        let mut sub = Subscription::default();
        sub.pin(p("Song"));
        assert_eq!(
            decide(&p("Song/Audio Files/kick.wav"), &m, &sub).hydration,
            Hydration::Resident,
            "otherwise a pinned session arrives without its audio"
        );
    }

    #[test]
    fn unpinning_returns_it_to_the_subscription() {
        let m = album();
        let mut sub = Subscription::new([Facet::new("sessions")]);
        sub.pin(p("Proxies/reel.mov"));
        sub.unpin(&p("Proxies/reel.mov"));
        assert_eq!(
            decide(&p("Proxies/reel.mov"), &m, &sub).hydration,
            Hydration::Stub
        );
    }

    #[test]
    fn unmapped_is_a_stub_rather_than_a_guess() {
        let m = album();
        let sub = Subscription::new([Facet::new("sessions")]);
        let d = decide(&p("Video ISO Files/iso1.mov"), &m, &sub);
        assert_eq!(d.hydration, Hydration::Stub);
        assert_eq!(d.reason, Reason::NotSubscribed);
    }

    #[test]
    fn unmapped_follows_a_project_default_when_there_is_one() {
        let mut m = album();
        m.with_default(Facet::new("misc"));
        let sub = Subscription::new([Facet::new("misc")]);
        assert_eq!(
            decide(&p("Video ISO Files/iso1.mov"), &m, &sub).hydration,
            Hydration::Resident
        );
    }

    #[test]
    fn a_project_mapping_is_subscribable_like_any_other() {
        let m = album();
        let sub = Subscription::new([Facet::new("assembly")]);
        let d = decide(&p("Project Assembly/Click TIME.wav"), &m, &sub);
        assert_eq!(d.hydration, Hydration::Resident);
        assert_eq!(d.reason, Reason::Facet(Facet::new("assembly")));
    }

    #[test]
    fn everything_takes_everything() {
        let m = album();
        let sub = Subscription::everything();
        for path in ["Proxies/a.mov", "Song/Audio Files/b.wav", "Whatever/c.bin"] {
            assert_eq!(decide(&p(path), &m, &sub).hydration, Hydration::Resident);
        }
    }

    #[test]
    // t[verify files.sync.selective]
    fn nothing_is_ever_absent() {
        let m = album();
        let sub = Subscription::default();
        let paths: Vec<RootPath> = ["a.wav", "Proxies/b.mov", "Song/Audio Files/c.wav"]
            .iter()
            .map(|s| p(s))
            .collect();
        let plan = plan(paths.iter(), &m, &sub);
        assert_eq!(plan.len(), 3);
        assert!(
            plan.iter()
                .all(|(_, d)| d.hydration == Hydration::Stub),
            "unsubscribed is a stub — present and sized — never missing"
        );
    }
}
