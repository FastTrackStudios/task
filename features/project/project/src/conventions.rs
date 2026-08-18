//! What holding a capability means — `project.capability.conventions`.
//!
//! `project.capability.multiple` is a project declaring *what it does*;
//! this is what follows from having said so. The rule lists five
//! consequences — facets, artifacts to ignore or absorb, checkpoint
//! cadence, deliverable kinds, UI surfaces — and the first two already
//! existed in `files_domain` before this module did, driving the ignore
//! layer and the facet map.
//!
//! # Two enums, and why that is not a mistake being repeated
//!
//! [`project_proto::Capability`] and [`files_domain::facet::Capability`]
//! are the same vocabulary in two crates. That is a duplication, and
//! duplication is what `project.definition.single` exists to prevent, so
//! it is worth saying exactly why it stands.
//!
//! The two crates cannot see each other and should not. `files-domain`
//! must not depend on `project` — a device serving a replica has no
//! projects and needs the ignore set anyway — and `project-proto` is
//! wasm-clean and deliberately light, while `files-domain` carries the
//! chunk store and the wire model. Neither direction is available and
//! there is no shared leaf to move it to.
//!
//! What stands in for sharing is [`tests::the_two_vocabularies_are_one`]:
//! it fails the moment either enum gains a member the other lacks. A
//! duplication that cannot drift silently is a different thing from a
//! duplication.
//!
//! # Three consequences are not here yet
//!
//! Checkpoint cadence lives in `files_domain::cadence` and is not yet
//! keyed by capability. Deliverable kinds and UI surfaces have no home
//! at all. Each is named in the rule and each is unimplemented, which is
//! why `project.capability.conventions` carries an impl marker for the
//! half that exists and this paragraph for the half that does not.

use files_domain::facet::Capability as Convention;
use project_proto::parts::{Capabilities, Capability};

/// The conventions half of a capability — what `files` already knows.
#[must_use]
pub fn convention_of(capability: Capability) -> Convention {
    match capability {
        Capability::MusicProduction => Convention::MusicProduction,
        Capability::VideoProduction => Convention::VideoProduction,
    }
}

// t[impl project.capability.conventions] — the facets a capability's work
// produces and the artifacts it leaves behind, which is the half of the
// rule that has an implementation. Cadence, deliverable kinds and UI
// surfaces are named by the rule and do not exist yet; see the module docs
/// Every convention a project's declared capabilities bring.
///
/// The order is the declaration's, so a project holding music and video
/// gets music's conventions first — which matters nowhere yet and will
/// the moment two capabilities disagree about a directory.
#[must_use]
pub fn conventions(capabilities: &Capabilities) -> Vec<Convention> {
    capabilities
        .held
        .iter()
        .copied()
        .map(convention_of)
        .collect()
}

/// The ignore set a project's capabilities imply.
///
/// `files.ignore.layers`' capability layer, selected by what the project
/// says it does rather than by what its files look like. A project that
/// declares nothing gets the platform layer and no more, which is the
/// correct answer rather than a degraded one: we do not know what tool
/// wrote this, so we do not guess which of its files are droppings.
#[must_use]
pub fn ignores(capabilities: &Capabilities) -> files_domain::ignore::IgnoreSet {
    files_domain::ignore::IgnoreSet::new(conventions(capabilities))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two vocabularies are one vocabulary.
    ///
    /// This is the test that makes the duplication in the module docs
    /// survivable. Add a capability to either enum without the other and
    /// it fails here, naming the member that has no counterpart —
    /// instead of a project declaring something the ignore layer has
    /// never heard of, which surfaces months later as a DAW's backup
    /// files appearing in somebody's listing.
    #[test]
    fn the_two_vocabularies_are_one() {
        // Every declaration has a convention. Total by construction —
        // `convention_of` is exhaustive — so what this really checks is
        // the other direction, below.
        let declared: Vec<Convention> = Capability::all()
            .iter()
            .copied()
            .map(convention_of)
            .collect();

        // Every convention is declarable. `files_domain` has no `all()`,
        // so this is written out: the point is that adding a variant
        // there makes this list fail to compile as non-exhaustive, and
        // the reader has to come here and decide.
        for convention in [Convention::MusicProduction, Convention::VideoProduction] {
            assert!(
                declared.contains(&convention),
                "`files_domain` knows a capability that no project can \
                 declare: {convention:?}"
            );
        }
        assert_eq!(
            declared.len(),
            2,
            "a capability was added to `project_proto` without a \
             convention in `files_domain`"
        );
    }

    /// A declared capability brings its tool's artifacts with it.
    // t[verify project.capability.conventions] — the half that exists:
    // a declared capability brings its tool's artifacts with it, and
    // declaring nothing does not guess
    #[test]
    fn declaring_music_production_ignores_a_daws_backups() {
        use files_domain::ignore::Layer;
        let music = ignores(&Capabilities::from_names(["music-production"]));
        assert_eq!(music.ignored("Song.rpp-bak"), Some(Layer::Capability));

        // And declaring nothing does not guess.
        let none = ignores(&Capabilities::default());
        assert_eq!(none.ignored("Song.rpp-bak"), None);
        // The platform layer applies regardless — it is not a capability
        // convention, it is what an operating system leaves behind.
        assert_eq!(none.ignored(".DS_Store"), Some(Layer::Platform));
    }
}
