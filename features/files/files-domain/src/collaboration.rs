//! Which orgs are on a project — `project.location.*`.
//!
//! A project is not owned by one company. An album is tracked by a
//! studio, mixed by an engineer who bills through their own outfit, cut
//! by a video company for the release, and delivered to the artist's
//! label. Four orgs, one project, and no honest way to pick the one that
//! "has" it.
//!
//! # The origin is a storage default and nothing else
//!
//! Something has to be the answer to "where do bytes land when nobody
//! said". That is the origin: the org that started the project, so its
//! disk is where new content goes by default.
//!
//! It is **not** ownership. The origin holds no authority the other
//! members lack — cannot remove them, cannot revoke their access, is not
//! required for a transfer between two of them, and can leave. Saying so
//! in a doc comment is not enough, so this type has no method that grants
//! the origin anything, and the tests below assert the absences.
//!
//! Why so insistent: the tree this was written against records the
//! collaboration in two fields that were never designed to hold it — an
//! `organization:` line and an `org:` tag — and eight projects use them to
//! mean "whose work it is" and "who is doing it" respectively while
//! sitting on a *third* org's disk. Three different facts, two fields,
//! and the only reason it works is that a person is reading them. A model
//! that collapses them has to throw two away, and the one it would keep
//! is the least useful: the directory.
//!
//! # What this is not
//!
//! - **Not placement.** Where content physically sits is
//!   `files.peering`'s admitted-host set, and any member's disk is a
//!   legitimate answer.
//! - **Not access.** What a *person* may do is a grant over a path
//!   (`files.access.granularity`). Being in a collaborating org is not a
//!   capability.
//! - **Not the client.** The artist a project is *for* is usually not an
//!   org at all, and the folder name is where a studio writes it down —
//!   see [`crate::layout::Project::clients`].

use std::collections::BTreeSet;

use crate::peering::OrgId;

/// Why a change to a collaboration was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CollabError {
    #[error("{0}: not on this project")]
    NotAMember(String),
    #[error(
        "{0} is the origin and the only member; a project with nobody on it \
         has no default location and nowhere for content to go"
    )]
    LastMember(String),
}

/// The orgs on one project.
#[derive(Debug, Clone, PartialEq, Eq, facet::Facet)]
#[repr(C)]
pub struct Collaboration {
    /// Where new content lands when nobody said otherwise. Always a
    /// member — a default pointing at a company that left is worse than
    /// no default, because it looks like an answer.
    origin: OrgId,
    /// Every org on it, the origin included.
    members: BTreeSet<OrgId>,
}

impl Collaboration {
    /// A project one org has started alone.
    #[must_use]
    pub fn started_by(origin: OrgId) -> Self {
        let mut members = BTreeSet::new();
        members.insert(origin.clone());
        Self { origin, members }
    }

    /// Bring an org onto the project.
    ///
    /// Idempotent, and returns `&mut Self` so a collaboration reads as
    /// the list it is.
    pub fn joined_by(&mut self, org: OrgId) -> &mut Self {
        self.members.insert(org);
        self
    }

    /// Take an org off the project.
    ///
    /// The origin may leave like anyone else — a project outlives the
    /// company that started it, and a studio that closes should not take
    /// its clients' work with it. What it cannot do is leave *last*,
    /// because the default location would then name nobody.
    ///
    /// When the origin leaves, the default moves to whichever member
    /// sorts first. Deterministic rather than arbitrary: two hosts folding
    /// the same change must reach the same answer, and "first
    /// alphabetically" is a rule both can apply without asking.
    pub fn left_by(&mut self, org: &OrgId) -> Result<(), CollabError> {
        if !self.members.contains(org) {
            return Err(CollabError::NotAMember(org.0.clone()));
        }
        if self.members.len() == 1 {
            return Err(CollabError::LastMember(org.0.clone()));
        }
        self.members.remove(org);
        if &self.origin == org {
            self.origin = self
                .members
                .iter()
                .next()
                .cloned()
                .expect("checked non-empty above");
        }
        Ok(())
    }

    /// Move the default location to another member.
    ///
    /// The whole of what "handing over a project" means here. Nothing else
    /// changes: the same orgs are on it, with the same standing, and any
    /// content already on the old origin's disk stays exactly where it is
    /// — placement is `files.peering`'s business and this moves a default,
    /// not bytes.
    pub fn hand_over(&mut self, to: &OrgId) -> Result<(), CollabError> {
        if !self.members.contains(to) {
            return Err(CollabError::NotAMember(to.0.clone()));
        }
        self.origin = to.clone();
        Ok(())
    }

    /// Where content goes when nobody said.
    ///
    /// Named for what it does. There is no `owner()`, and that is the
    /// point: a caller reaching for one would find this and have to decide
    /// whether a default location is really what it wanted.
    #[must_use]
    pub fn default_location(&self) -> &OrgId {
        &self.origin
    }

    /// The org that started it — the same value as
    /// [`Self::default_location`], under the name that says *why* it is
    /// the default rather than what the default is for.
    #[must_use]
    pub fn origin(&self) -> &OrgId {
        &self.origin
    }

    /// Every org on the project, sorted.
    pub fn members(&self) -> impl Iterator<Item = &OrgId> {
        self.members.iter()
    }

    #[must_use]
    pub fn includes(&self, org: &OrgId) -> bool {
        self.members.contains(org)
    }

    /// How many orgs are on it. `1` is an ordinary project, not a
    /// degenerate one — most work is one company's.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        false // an origin is always a member
    }

    /// Whether more than one org is on this.
    ///
    /// The question a UI asks before showing a collaborator list, and the
    /// one a transfer asks before assuming both ends are the same company.
    #[must_use]
    pub fn is_shared(&self) -> bool {
        self.members.len() > 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org(name: &str) -> OrgId {
        OrgId(name.to_string())
    }

    #[test]
    fn a_project_starts_with_one_org_on_it() {
        let c = Collaboration::started_by(org("acme-audio"));
        assert_eq!(c.default_location(), &org("acme-audio"));
        assert_eq!(c.len(), 1);
        assert!(!c.is_shared(), "one company is not a collaboration");
    }

    /// The real shape: a project on one org's disk, worked by another.
    #[test]
    fn several_orgs_can_be_on_one_project() {
        let mut c = Collaboration::started_by(org("vnt-video"));
        c.joined_by(org("acme-audio"));

        assert!(c.includes(&org("acme-audio")));
        assert!(c.includes(&org("vnt-video")));
        assert!(c.is_shared());
        // And the origin is still only the default.
        assert_eq!(c.default_location(), &org("vnt-video"));
    }

    #[test]
    fn joining_twice_changes_nothing() {
        let mut c = Collaboration::started_by(org("acme-audio"));
        c.joined_by(org("vnt-video")).joined_by(org("vnt-video"));
        assert_eq!(c.len(), 2);
    }

    /// The origin confers nothing, and the way to assert that is to
    /// enumerate what a member can do and find no difference.
    #[test]
    fn the_origin_holds_no_authority_over_the_others() {
        let mut c = Collaboration::started_by(org("vnt-video"));
        c.joined_by(org("acme-audio"));

        // Either can leave.
        let mut a = c.clone();
        a.left_by(&org("acme-audio")).expect("a member may leave");
        let mut b = c.clone();
        b.left_by(&org("vnt-video")).expect("the origin may leave too");

        // Either can be handed the default.
        let mut d = c.clone();
        d.hand_over(&org("acme-audio")).expect("handing over");
        assert_eq!(d.default_location(), &org("acme-audio"));
    }

    /// A studio that closes should not take its clients' work with it.
    #[test]
    fn when_the_origin_leaves_the_default_moves() {
        let mut c = Collaboration::started_by(org("vnt-video"));
        c.joined_by(org("acme-audio"));

        c.left_by(&org("vnt-video")).expect("the origin leaves");

        assert_eq!(
            c.default_location(),
            &org("acme-audio"),
            "the default named a company that had left"
        );
        assert!(!c.includes(&org("vnt-video")));
    }

    /// …but it cannot leave last, because then nothing answers "where
    /// does content go".
    #[test]
    fn the_last_org_cannot_leave() {
        let mut c = Collaboration::started_by(org("acme-audio"));
        assert_eq!(
            c.left_by(&org("acme-audio")),
            Err(CollabError::LastMember("acme-audio".into()))
        );
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn a_stranger_can_neither_leave_nor_be_handed_the_default() {
        let mut c = Collaboration::started_by(org("acme-audio"));
        assert!(matches!(
            c.left_by(&org("stranger")),
            Err(CollabError::NotAMember(_))
        ));
        assert!(matches!(
            c.hand_over(&org("stranger")),
            Err(CollabError::NotAMember(_))
        ));
        assert_eq!(c.default_location(), &org("acme-audio"));
    }

    /// Two hosts folding the same departure must reach the same default,
    /// or they disagree about where content goes and neither is wrong.
    #[test]
    fn the_new_default_is_the_same_on_every_host() {
        let build = || {
            let mut c = Collaboration::started_by(org("vnt-video"));
            c.joined_by(org("zebra-post")).joined_by(org("acme-audio"));
            c
        };
        let mut one = build();
        let mut two = build();
        one.left_by(&org("vnt-video")).unwrap();
        two.left_by(&org("vnt-video")).unwrap();
        assert_eq!(one.default_location(), two.default_location());
        assert_eq!(one.default_location(), &org("acme-audio"), "sorted first");
    }
}
