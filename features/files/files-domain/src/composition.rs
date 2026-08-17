//! One project, many locations — `project.location.composed`.
//!
//! A project's content does not have to live in one place, and the rule
//! is explicit that this is the normal case rather than a degraded one:
//! a video company's server holds the footage while an audio company's
//! holds the sessions, and both are the same project. One tree, one set
//! of deliverables, one identity.
//!
//! # No location is privileged
//!
//! That clause is the whole design. A composition is a *set* of members,
//! not a home plus some attachments — there is no field for "the real
//! one", because the moment one exists every other member is a guest in
//! its own project and losing it means losing the project rather than a
//! part of it.
//!
//! So resolution is by name, and the members are ordered only for
//! display. Adding or removing one changes where bytes live and not what
//! the project is.
//!
//! # This lives here because it composes Files roots
//!
//! The declaration belongs to the project feature — which project, whose
//! capabilities, which parts. What is here is only the mapping from a
//! path in the composed tree to the root that answers for it, which is a
//! statement about roots and is testable without a project at all.

use std::collections::BTreeMap;

use files_proto::id::RootId;
use files_proto::path::RootPath;

/// One place a project's content lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// What this part is called in the composed tree. The name a person
    /// uses — "Sessions", "Footage" — not an id.
    pub name: String,
    /// The root answering for it. A root accepted from another server is
    /// an ordinary `RootId` here, exactly as it is everywhere else, so
    /// composition needs to know nothing about federation.
    pub root: RootId,
    /// The subtree within that root. Empty means the whole of it.
    pub path: RootPath,
}

/// Where a path in the composed tree actually is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located<'a> {
    pub member: &'a Member,
    /// The path to ask that member's root for.
    pub within: RootPath,
}

/// Why a composed path did not resolve.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ComposeError {
    #[error("{0}: no such part of this project")]
    NoSuchMember(String),
    #[error("a part needs a name")]
    Unnamed,
    #[error("{0}: two parts cannot share a name")]
    Duplicate(String),
    #[error("{0}")]
    BadPath(#[from] files_proto::path::PathError),
}

/// A project's content, wherever it lives.
#[derive(Debug, Clone, Default)]
pub struct Composition {
    members: BTreeMap<String, Member>,
}

impl Composition {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a place this project's content lives.
    ///
    /// Names are unique because they are how a path resolves; two parts
    /// called `Sessions` would make `Sessions/mix.wav` ambiguous, and
    /// picking one silently is how a caller ends up reading the wrong
    /// company's file.
    pub fn with(&mut self, member: Member) -> Result<&mut Self, ComposeError> {
        if member.name.trim().is_empty() {
            return Err(ComposeError::Unnamed);
        }
        if self.members.contains_key(&member.name) {
            return Err(ComposeError::Duplicate(member.name));
        }
        self.members.insert(member.name.clone(), member);
        Ok(self)
    }

    /// Drop a part.
    ///
    /// Changes where bytes live, never what the project is — which is
    /// why this returns whether anything was there rather than an error.
    pub fn without(&mut self, name: &str) -> bool {
        self.members.remove(name).is_some()
    }

    /// The parts, in display order.
    pub fn members(&self) -> impl Iterator<Item = &Member> {
        self.members.values()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// How many distinct roots this project draws on.
    ///
    /// More than one is the ordinary case, and a UI that wants to say
    /// "across 2 locations" asks here.
    #[must_use]
    pub fn locations(&self) -> usize {
        let mut roots: Vec<RootId> = self.members.values().map(|m| m.root).collect();
        roots.sort_unstable();
        roots.dedup();
        roots.len()
    }

    /// Resolve a path in the composed tree.
    ///
    /// The root of the composition is the list of parts, so `""` resolves
    /// to nothing in particular and callers list [`Self::members`]
    /// instead. Anything deeper names a part first.
    pub fn locate(&self, path: &RootPath) -> Result<Located<'_>, ComposeError> {
        let mut parts = path.components();
        let Some(head) = parts.next() else {
            return Err(ComposeError::NoSuchMember(String::new()));
        };
        let member = self
            .members
            .get(head)
            .ok_or_else(|| ComposeError::NoSuchMember(head.to_string()))?;

        let rest: Vec<&str> = parts.collect();
        // The member's own subtree is a prefix the caller never sees: a
        // project part that points at `Projects/Album/Audio` is browsed
        // as `Sessions/…`, because where a company files its work is not
        // the collaborator's business.
        let within = match (member.path.is_root(), rest.is_empty()) {
            (true, true) => RootPath::root(),
            (true, false) => RootPath::parse(rest.join("/"))?,
            (false, true) => member.path.clone(),
            (false, false) => RootPath::parse(format!("{}/{}", member.path, rest.join("/")))?,
        };
        Ok(Located { member, within })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, root: u8, path: &str) -> Member {
        Member {
            name: name.into(),
            root: RootId::new(uuid::Uuid::from_bytes([root; 16])),
            path: RootPath::parse(path).unwrap(),
        }
    }

    fn album() -> Composition {
        let mut c = Composition::new();
        // The audio company's sessions and the video company's footage,
        // on different servers, in one project.
        c.with(member("Sessions", 1, "")).unwrap();
        c.with(member("Footage", 2, "Proxies")).unwrap();
        c
    }

    // t[verify project.location.composed]
    #[test]
    fn a_project_draws_on_more_than_one_location() {
        let c = album();
        assert_eq!(c.locations(), 2);
        assert_eq!(c.members().count(), 2);
    }

    // t[verify project.location.composed]
    #[test]
    fn no_member_is_the_projects_real_home() {
        // Removing either leaves a project, not a broken one. There is no
        // field to consult for "the real one" because there is no such
        // thing.
        let mut a = album();
        a.without("Sessions");
        assert_eq!(a.locations(), 1);

        let mut b = album();
        b.without("Footage");
        assert_eq!(b.locations(), 1);
        assert!(!b.is_empty());
    }

    #[test]
    fn a_path_resolves_to_the_part_that_answers_for_it() {
        let c = album();
        let found = c
            .locate(&RootPath::parse("Sessions/mix.wav").unwrap())
            .unwrap();
        assert_eq!(found.member.name, "Sessions");
        assert_eq!(found.within.as_str(), "mix.wav");
    }

    #[test]
    fn a_members_own_subtree_is_a_prefix_the_caller_never_sees() {
        // `Footage` points at `Proxies` inside its root. A collaborator
        // browsing `Footage/reel.mov` should not have to know that, or
        // where the video company files its work.
        let c = album();
        let found = c
            .locate(&RootPath::parse("Footage/reel.mov").unwrap())
            .unwrap();
        assert_eq!(found.within.as_str(), "Proxies/reel.mov");
    }

    #[test]
    fn naming_a_part_that_is_not_there_is_not_a_traversal() {
        let c = album();
        assert!(matches!(
            c.locate(&RootPath::parse("Elsewhere/secret").unwrap()),
            Err(ComposeError::NoSuchMember(name)) if name == "Elsewhere"
        ));
    }

    #[test]
    fn two_parts_cannot_share_a_name() {
        let mut c = album();
        // Otherwise `Sessions/mix.wav` is ambiguous, and picking one
        // silently is how a caller reads the wrong company's file.
        assert!(matches!(
            c.with(member("Sessions", 3, "")),
            Err(ComposeError::Duplicate(_))
        ));
    }

    #[test]
    fn a_part_needs_a_name() {
        let mut c = Composition::new();
        assert!(matches!(
            c.with(member("  ", 1, "")),
            Err(ComposeError::Unnamed)
        ));
    }
}
