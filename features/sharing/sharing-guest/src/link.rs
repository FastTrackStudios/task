//! Link redaction — what a guest sees where a record points outside
//! its grant.
//!
//! t[impl sharing.grant.exclusions] — "a link to an out-of-scope object
//! resolves to nothing". Every reference a guest view carries (a parent
//! project, a task relation, a setlist entry, an inbox item's
//! `processed_into`) goes through this module. A reference inside the
//! grant passes through unchanged as [`Linked::Shared`]. A reference
//! outside it becomes [`Linked::NotShared`], which carries **nothing**:
//! no id, no title, no kind beyond what the field already says.
//!
//! t[impl sharing.grant.absent] — the marker is the only trace. A list
//! that points outside the grant three times shows the marker once, in
//! the position of the first such entry, so the guest cannot count how
//! much of the owner's world sits behind it ("no id, title or count").
//!
//! The module is pure. Whether an object is in scope is decided by the
//! caller's predicate — the subtree resolver (S4) in the server, a
//! fixed [`ScopeSet`] in tests — so nothing here reads a store.

use std::collections::HashSet;

use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The text a surface renders for [`Linked::NotShared`].
pub const NOT_SHARED: &str = "linked: (not shared)";

/// One reference, as a guest receives it.
///
/// `NotShared` is a unit variant on purpose: there is no field in which
/// an id or a title could ride along by mistake. It serializes as the
/// bare string [`NOT_SHARED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum Linked<T> {
    /// The referenced object is inside the grant.
    #[serde(rename = "shared")]
    Shared(T),
    /// The referenced object is outside the grant. Nothing else is said.
    #[serde(rename = "linked: (not shared)")]
    NotShared,
}

impl<T> Linked<T> {
    /// `true` for the opaque marker.
    #[must_use]
    pub fn is_not_shared(&self) -> bool {
        matches!(self, Self::NotShared)
    }

    /// The shared value, if there is one.
    #[must_use]
    pub fn shared(&self) -> Option<&T> {
        match self {
            Self::Shared(t) => Some(t),
            Self::NotShared => None,
        }
    }
}

/// What a reference points at, as handed to the scope predicate.
///
/// The variant says what kind of object the *field* names; the
/// predicate answers whether that object is inside the grant. A kind
/// the grant can never include (a cycle, a workstream, another org's
/// project) is still asked rather than assumed, so the resolver stays
/// the single place that decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectRef<'a> {
    /// A project, by id (`parent_id`, `project_id`, `processed_into =
    /// "project:<id>"`).
    Project(Uuid),
    /// A part or piece, by id — the same id before and after promotion
    /// (`Part::references`, setlist entries, `DeliverableItem::part`).
    Piece(Uuid),
    /// A task, by id (relations, blockers, `parent`, reverse relations).
    Task(Uuid),
    /// A milestone, by id (`TaskInfo::milestone_id`).
    Milestone(Uuid),
    /// A cycle / sprint, by id (`WorkflowAttrs::cycle`).
    Cycle(Uuid),
    /// A workstream, by id (`WorkflowAttrs::workstream`).
    Workstream(Uuid),
    /// A project named by wikilink text (`TaskInfo::projects`).
    ProjectWikilink(&'a str),
    /// A federation pointer to another org's project
    /// (`ProjectInfo::same_as`, `@org/slug`).
    Federated(&'a str),
    /// A vault-relative path — a task page or a note
    /// (`InboxItem::processed_into` when it is not `project:<id>`).
    Path(&'a str),
}

/// Redact one reference.
pub fn redact_one<'a, T, S>(value: T, r: ObjectRef<'a>, in_scope: &S) -> Linked<T>
where
    S: Fn(ObjectRef<'_>) -> bool + ?Sized,
{
    if in_scope(r) {
        Linked::Shared(value)
    } else {
        Linked::NotShared
    }
}

/// Redact an optional reference. `None` stays `None` — "no parent" is
/// not a link, so it is not hidden.
pub fn redact_opt<T, S>(
    value: Option<T>,
    to_ref: impl Fn(&T) -> ObjectRef<'_>,
    in_scope: &S,
) -> Option<Linked<T>>
where
    S: Fn(ObjectRef<'_>) -> bool + ?Sized,
{
    value.map(|v| {
        let keep = in_scope(to_ref(&v));
        if keep {
            Linked::Shared(v)
        } else {
            Linked::NotShared
        }
    })
}

/// Redact a list of references.
///
/// In-scope entries keep their order. Every out-of-scope entry collapses
/// into **one** [`Linked::NotShared`] at the position of the first, so
/// the result reveals that something outside the grant is linked and
/// nothing about how much (t[impl sharing.grant.exclusions]).
pub fn redact_list<T, S>(
    items: impl IntoIterator<Item = T>,
    to_ref: impl Fn(&T) -> ObjectRef<'_>,
    in_scope: &S,
) -> Vec<Linked<T>>
where
    S: Fn(ObjectRef<'_>) -> bool + ?Sized,
{
    let mut out = Vec::new();
    let mut marked = false;
    for item in items {
        if in_scope(to_ref(&item)) {
            out.push(Linked::Shared(item));
        } else if !marked {
            marked = true;
            out.push(Linked::NotShared);
        }
    }
    out
}

/// The reverse-relation index entry for one task, redacted
/// (`TaskService::reverse_relations`).
pub fn redact_reverse_relations<S>(
    rels: impl IntoIterator<Item = task_proto::ReverseRelation>,
    in_scope: &S,
) -> Vec<Linked<task_proto::ReverseRelation>>
where
    S: Fn(ObjectRef<'_>) -> bool + ?Sized,
{
    redact_list(rels, |r| ObjectRef::Task(r.source), in_scope)
}

/// What `InboxItem::processed_into` points at: `project:<uuid>` names a
/// project, anything else is a vault path (a task page or a note).
#[must_use]
pub fn processed_into_ref(into: &str) -> ObjectRef<'_> {
    match into
        .strip_prefix("project:")
        .and_then(|s| s.parse::<Uuid>().ok())
    {
        Some(id) => ObjectRef::Project(id),
        None => ObjectRef::Path(into),
    }
}

/// Redact an inbox item's `processed_into` provenance pointer.
pub fn redact_processed_into<S>(into: Option<&str>, in_scope: &S) -> Option<Linked<String>>
where
    S: Fn(ObjectRef<'_>) -> bool + ?Sized,
{
    let into = into?;
    Some(redact_one(
        into.to_owned(),
        processed_into_ref(into),
        in_scope,
    ))
}

/// A fixed in-scope set — the shape the grant preview digest is taken
/// over, and the simplest predicate to hand the helpers above.
///
/// Ids are shared across kinds (a promoted part and its project carry
/// one id), so membership is by id regardless of variant. Wikilinks and
/// federation pointers are never in a set: a guest view must not
/// resolve a name, so those always redact unless a resolver says
/// otherwise.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeSet {
    pub ids: HashSet<Uuid>,
    pub paths: HashSet<String>,
}

impl ScopeSet {
    #[must_use]
    pub fn new(ids: impl IntoIterator<Item = Uuid>) -> Self {
        Self {
            ids: ids.into_iter().collect(),
            paths: HashSet::new(),
        }
    }

    #[must_use]
    pub fn with_paths<S: Into<String>>(mut self, paths: impl IntoIterator<Item = S>) -> Self {
        self.paths.extend(paths.into_iter().map(Into::into));
        self
    }

    /// The predicate form the redaction helpers take.
    #[must_use]
    pub fn contains(&self, r: ObjectRef<'_>) -> bool {
        match r {
            ObjectRef::Project(id)
            | ObjectRef::Piece(id)
            | ObjectRef::Task(id)
            | ObjectRef::Milestone(id)
            | ObjectRef::Cycle(id)
            | ObjectRef::Workstream(id) => self.ids.contains(&id),
            ObjectRef::Path(p) => self.paths.contains(p),
            ObjectRef::ProjectWikilink(_) | ObjectRef::Federated(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use task_proto::{RelationKind, ReverseRelation};

    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    // t[verify sharing.grant.exclusions]
    #[test]
    fn an_out_of_scope_link_is_the_bare_marker_with_no_id_or_title() {
        let secret = id(0xdead_beef);
        let scope = ScopeSet::new([id(1)]);
        let got = redact_one(
            ("Other Client Album".to_owned(), secret),
            ObjectRef::Project(secret),
            &|r| scope.contains(r),
        );
        assert_eq!(got, Linked::NotShared);
        let json = serde_json::to_string(&got).unwrap();
        assert_eq!(json, format!("\"{NOT_SHARED}\""));
        assert!(!json.contains(&secret.to_string()));
        assert!(!json.contains("Other Client"));
    }

    #[test]
    fn an_in_scope_link_passes_through_unchanged() {
        let scope = ScopeSet::new([id(1)]);
        let got = redact_one(id(1), ObjectRef::Task(id(1)), &|r| scope.contains(r));
        assert_eq!(got, Linked::Shared(id(1)));
    }

    // t[verify sharing.grant.absent]
    #[test]
    fn many_out_of_scope_links_collapse_to_one_marker_so_nothing_is_counted() {
        let scope = ScopeSet::new([id(1), id(3)]);
        let got = redact_list([id(9), id(1), id(8), id(3), id(7)], |u| ObjectRef::Task(*u), &|r| {
            scope.contains(r)
        });
        assert_eq!(
            got,
            vec![
                Linked::NotShared,
                Linked::Shared(id(1)),
                Linked::Shared(id(3)),
            ]
        );
        let none_out = redact_list([id(1)], |u| ObjectRef::Task(*u), &|r| scope.contains(r));
        assert_eq!(none_out, vec![Linked::Shared(id(1))]);
    }

    #[test]
    fn reverse_relations_from_outside_the_grant_leak_no_source() {
        let scope = ScopeSet::new([id(1)]);
        let rels = vec![
            ReverseRelation {
                kind: RelationKind::Blocks,
                source: id(1),
            },
            ReverseRelation {
                kind: RelationKind::Relates,
                source: id(0xfeed),
            },
            ReverseRelation {
                kind: RelationKind::Blocks,
                source: id(0xf00d),
            },
        ];
        let got = redact_reverse_relations(rels, &|r| scope.contains(r));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].shared().map(|r| r.source), Some(id(1)));
        assert!(got[1].is_not_shared());
        let json = serde_json::to_string(&got).unwrap();
        assert!(!json.contains(&id(0xfeed).to_string()));
        assert!(!json.contains(&id(0xf00d).to_string()));
    }

    #[test]
    fn processed_into_redacts_projects_and_paths_outside_the_grant() {
        let inside = id(1);
        let outside = id(2);
        let scope = ScopeSet::new([inside]).with_paths(["Projects/Crescendum/tasks/mix.md"]);
        let pred = |r: ObjectRef<'_>| scope.contains(r);

        let p_in = format!("project:{inside}");
        assert_eq!(
            redact_processed_into(Some(&p_in), &pred),
            Some(Linked::Shared(p_in.clone()))
        );
        let p_out = format!("project:{outside}");
        let got = redact_processed_into(Some(&p_out), &pred);
        assert_eq!(got, Some(Linked::NotShared));
        assert!(
            !serde_json::to_string(&got)
                .unwrap()
                .contains(&outside.to_string())
        );

        assert_eq!(
            redact_processed_into(Some("Projects/Crescendum/tasks/mix.md"), &pred),
            Some(Linked::Shared("Projects/Crescendum/tasks/mix.md".to_owned()))
        );
        let got = redact_processed_into(Some("Wiki/Private Rates.md"), &pred);
        assert_eq!(got, Some(Linked::NotShared));
        assert!(!serde_json::to_string(&got).unwrap().contains("Private"));

        assert_eq!(redact_processed_into(None, &pred), None);
    }

    #[test]
    fn names_are_never_resolved_by_a_fixed_set() {
        let scope = ScopeSet::new([id(1)]);
        assert!(!scope.contains(ObjectRef::ProjectWikilink("[[Crescendum]]")));
        assert!(!scope.contains(ObjectRef::Federated("@fta/crescendum")));
    }

    #[test]
    fn the_predicate_is_an_argument_so_a_resolver_plugs_in() {
        // S4's resolver is any closure; here, "every task, no projects".
        let resolver = |r: ObjectRef<'_>| matches!(r, ObjectRef::Task(_));
        assert_eq!(
            redact_one((), ObjectRef::Task(id(5)), &resolver),
            Linked::Shared(())
        );
        assert_eq!(
            redact_one((), ObjectRef::Project(id(5)), &resolver),
            Linked::NotShared
        );
    }
}
