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

use std::collections::{BTreeMap, BTreeSet};

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
    /// Referenced by a session the device keeps — the session's media,
    /// wherever in the root it sits (see [`decide_with`]).
    Referenced(RootPath),
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
/// ancestor — and, for a file nothing classified, from what it is: a
/// session file belongs to the atomic `sessions` facet wherever it sits.
///
/// The session check comes last on purpose. A directory's binding is the
/// more specific statement: a Pro Tools backup in `Session File Backups`
/// is a backup, and a session a project filed under its own mapping is
/// that mapping's.
fn classify(path: &RootPath, facets: &FacetMap) -> Option<crate::facet::Binding> {
    let mut probe = Some(path.clone());
    while let Some(p) = probe {
        let binding = facets.resolve(p.as_str());
        if binding.source != Source::Unmapped {
            return Some(binding);
        }
        probe = p.parent();
    }
    facets.session(path.as_str())
}

/// The media sessions reference, each mapped to the session that wants
/// it — the dependency half of an atomic facet.
///
/// Built by whoever can read session files (the lane does; this module
/// touches no disk) from the sessions this device keeps
/// ([`kept_sessions`]), and handed to [`decide_with`]. Only kept
/// sessions belong in it: a session the device does not keep brings
/// nothing, which is how a laptop that took one song is not handed the
/// whole album's audio.
#[derive(Debug, Clone, Default)]
pub struct References {
    by_media: BTreeMap<RootPath, RootPath>,
}

impl References {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `session` references each of `media`. When two
    /// sessions share a file the first one recorded is the one reported.
    pub fn add(&mut self, session: &RootPath, media: impl IntoIterator<Item = RootPath>) {
        for m in media {
            if &m != session {
                self.by_media.entry(m).or_insert_with(|| session.clone());
            }
        }
    }

    /// The kept session that wants `media`, if any does.
    #[must_use]
    pub fn session_of(&self, media: &RootPath) -> Option<&RootPath> {
        self.by_media.get(media)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_media.is_empty()
    }
}

/// The session files among `files` that this device keeps resident.
///
/// A session is kept by the same rules as anything else — pinned,
/// subscribed by facet, or taken with everything — so pinning
/// `Song/Song.RPP` is how one session is selected without taking every
/// other session in the project.
pub fn kept_sessions<'a, I>(files: I, facets: &FacetMap, sub: &Subscription) -> Vec<&'a RootPath>
where
    I: IntoIterator<Item = &'a RootPath>,
{
    files
        .into_iter()
        .filter(|f| {
            facets.is_session(f.as_str()) && decide(f, facets, sub).hydration == Hydration::Resident
        })
        .collect()
}

/// [`decide`], plus the rule an atomic facet makes: what a kept session
/// references is resident with it, wherever in the root it sits.
///
/// A path the subscription already keeps keeps its own reason. A path
/// it does not, but a kept session references, is resident because of
/// that session — which is the case that matters: REAPER records media
/// by path, and a take in a shared samples folder no tool layout names
/// is as much the session as the `.rpp` is. Nothing else changes: an
/// unreferenced file follows the subscription, so footage beside the
/// session stays a stub.
#[must_use]
// t[impl files.sync.selective] — a kept session brings what it references
pub fn decide_with(
    path: &RootPath,
    facets: &FacetMap,
    sub: &Subscription,
    refs: &References,
) -> Decision {
    let own = decide(path, facets, sub);
    if own.hydration == Hydration::Resident {
        return own;
    }
    match refs.session_of(path) {
        Some(session) => Decision::resident(Reason::Referenced(session.clone())),
        None => own,
    }
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
            plan.iter().all(|(_, d)| d.hydration == Hydration::Stub),
            "unsubscribed is a stub — present and sized — never missing"
        );
    }

    /// A REAPER album: a session per song, footage nobody's music tool
    /// names, and a samples folder the sessions reach into.
    fn reaper_album() -> FacetMap {
        FacetMap::new([Capability::MusicProduction])
    }

    #[test]
    // t[verify files.sync.selective]
    fn a_session_file_is_in_the_sessions_facet_and_a_backup_is_not() {
        let m = album();
        let sub = Subscription::new([Facet::new("sessions")]);
        let rpp = decide(&p("Song/Song.RPP"), &m, &sub);
        assert_eq!(
            rpp.hydration,
            Hydration::Resident,
            "subscribing to sessions must keep the session file itself, not only its media"
        );
        let backup = decide(&p("Song/Session File Backups/Song.ptx"), &m, &sub);
        assert_eq!(
            backup.hydration,
            Hydration::Stub,
            "a backup directory's binding beats the file's extension"
        );
    }

    #[test]
    // t[verify files.sync.selective]
    fn a_pinned_session_brings_what_it_references_and_nothing_else() {
        let m = reaper_album();
        let mut sub = Subscription::default();
        sub.pin(p("Track One/Track One.RPP"));
        let files: Vec<RootPath> = [
            "Track One/Track One.RPP",
            "Track One/Audio Files/Vox_1.wav",
            "Track Two/Track Two.RPP",
            "Track Two/Audio Files/Vox_1.wav",
            "Shared Samples/clap.wav",
            "Footage/cam-a.mov",
        ]
        .iter()
        .map(|s| p(s))
        .collect();

        let kept = kept_sessions(files.iter(), &m, &sub);
        assert_eq!(kept, vec![&p("Track One/Track One.RPP")]);

        let mut refs = References::new();
        refs.add(
            kept[0],
            [
                p("Track One/Audio Files/Vox_1.wav"),
                p("Shared Samples/clap.wav"),
            ],
        );

        let d = |path: &str| decide_with(&p(path), &m, &sub, &refs);
        assert_eq!(d("Track One/Track One.RPP").reason, Reason::Pinned);
        assert_eq!(
            d("Shared Samples/clap.wav").reason,
            Reason::Referenced(p("Track One/Track One.RPP")),
            "media outside every tool directory still comes with its session"
        );
        assert_eq!(
            d("Track One/Audio Files/Vox_1.wav").hydration,
            Hydration::Resident
        );
        for other in [
            "Track Two/Track Two.RPP",
            "Track Two/Audio Files/Vox_1.wav",
            "Footage/cam-a.mov",
        ] {
            assert_eq!(d(other).hydration, Hydration::Stub, "{other}");
        }
    }

    #[test]
    fn references_of_a_session_nobody_keeps_bring_nothing() {
        let m = reaper_album();
        let sub = Subscription::default();
        let files = [p("Song/Song.RPP")];
        assert!(
            kept_sessions(files.iter(), &m, &sub).is_empty(),
            "an unselected session is not kept"
        );
        // And a References built from no kept session changes nothing.
        let refs = References::new();
        assert_eq!(
            decide_with(&p("Song/Media/take.wav"), &m, &sub, &refs).hydration,
            Hydration::Stub
        );
    }
}
