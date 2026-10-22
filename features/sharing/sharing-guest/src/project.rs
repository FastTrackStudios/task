//! The guest view of a project.
//!
//! [`guest_project`] is the one projection from [`ProjectInfo`] to what
//! a grant guest receives, and it carries every nested record a guest
//! can reach through it: parts, deliverables, and — through the
//! standalone helpers below — the `parts`, `pieces`, `setlist`,
//! `deliverables` and `deliverable_items` RPC returns.
//!
//! t[impl sharing.grant.exclusions] — money is never in scope. The
//! fields in [`STRIPPED_PROJECT_FIELDS`] do not exist on
//! [`GuestProject`], so there is nothing to forget to blank:
//!
//! - `client_id` — the billable client (a `timer_clients` row, i.e. a
//!   contact in the owner's finance books);
//! - `billable_default`, `currency`, `default_rate_cents` — the billing
//!   knobs of the rate cascade;
//! - `estimated_seconds` — the budget denominator (`project_detail::
//!   budget::budget_tile_value` renders "logged / estimated" from it;
//!   the logged and invoiced halves come from timer + finance, which the
//!   lane never mounts);
//! - `agent_profile`, `verify_command` — not money, but operator
//!   internals (which agent runs the work, and a shell command line); a
//!   guest who reads and comments has no use for them, and the lane is
//!   default-deny.
//!
//! t[impl sharing.grant.absent] — references out of the grant
//! (`parent_id` above the shared root, `same_as` into another org, a
//! setlist entry naming another project's song) become the opaque
//! [`Linked::NotShared`] marker, never an id or a title.
//!
//! **Compile-time guard.** Every projection here destructures its source
//! struct exhaustively, with no `..`. Adding a field to [`ProjectInfo`],
//! [`Part`], [`Deliverable`] or [`DeliverableItem`] breaks this crate's
//! build until the field is either carried or stripped here — and a
//! stripped one must then be named in [`STRIPPED_PROJECT_FIELDS`] or
//! the key-classification test fails.

use chrono::{DateTime, NaiveDate, Utc};
use facet::Facet;
use project_proto::{
    Capabilities, Component, Deliverable, DeliverableItem, Deliverables, Form, Part, Piece,
    ProjectInfo, StatesConfig, Tags,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::link::{Linked, ObjectRef, redact_list, redact_opt};

/// `ProjectInfo` fields (serde names) the guest view drops.
pub const STRIPPED_PROJECT_FIELDS: &[&str] = &[
    "clientId",
    "billableDefault",
    "currency",
    "defaultRateCents",
    "estimatedSeconds",
    "agentProfile",
    "verifyCommand",
];

/// `ProjectInfo` fields (serde names) the guest view carries, some of
/// them redacted. `path` and `details` are `#[serde(skip)]` on the
/// source and carried too.
pub const SHARED_PROJECT_FIELDS: &[&str] = &[
    "id",
    "title",
    "status",
    "priority",
    "projectType",
    "lead",
    "tags",
    "parentId",
    "sameAs",
    "targetDate",
    "progressPercent",
    "color",
    "image",
    "archived",
    "parts",
    "capabilities",
    "form",
    "deliverables",
    "states",
    "dateCreated",
    "dateModified",
];

/// A project as a grant guest receives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[serde(rename_all = "camelCase")]
pub struct GuestProject {
    pub id: Uuid,
    pub path: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub project_type: String,
    pub lead: String,
    pub tags: Tags,
    /// `None` for a top-level project; the marker when the parent is
    /// above the shared root.
    pub parent_id: Option<Linked<Uuid>>,
    /// Federation pointer; the marker unless the resolver admits it.
    pub same_as: Option<Linked<String>>,
    pub target_date: Option<NaiveDate>,
    pub progress_percent: i16,
    pub details: String,
    pub color: String,
    pub image: String,
    pub archived: bool,
    pub parts: Vec<GuestPart>,
    pub capabilities: Capabilities,
    pub form: Option<Form>,
    pub deliverables: Deliverables,
    pub states: Option<StatesConfig>,
    pub date_created: Option<DateTime<Utc>>,
    pub date_modified: Option<DateTime<Utc>>,
}

/// A part as a guest receives it: a setlist entry's `references` is
/// redacted when the referenced piece is outside the grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[serde(rename_all = "camelCase")]
pub struct GuestPart {
    pub id: Uuid,
    pub name: String,
    pub references: Option<Linked<Uuid>>,
    pub components: Vec<Component>,
}

/// Project a [`ProjectInfo`] into its guest view.
///
/// Pure: `in_scope` is the grant's subtree predicate (the S4 resolver in
/// the server). See the module docs for what is stripped and why.
pub fn guest_project<S>(project: ProjectInfo, in_scope: &S) -> GuestProject
where
    S: Fn(ObjectRef<'_>) -> bool + ?Sized,
{
    // Exhaustive on purpose — no `..`. A new field must be decided here.
    let ProjectInfo {
        id,
        path,
        title,
        status,
        priority,
        project_type,
        lead,
        tags,
        parent_id,
        same_as,
        target_date,
        progress_percent,
        details,
        // ── stripped: money (t[impl sharing.grant.exclusions]) ──
        client_id: _,
        billable_default: _,
        currency: _,
        default_rate_cents: _,
        estimated_seconds: _,
        // ── stripped: operator internals ──
        agent_profile: _,
        verify_command: _,
        // ── carried ──
        color,
        image,
        archived,
        parts,
        capabilities,
        form,
        deliverables,
        states,
        date_created,
        date_modified,
    } = project;

    GuestProject {
        id,
        path,
        title,
        status,
        priority,
        project_type,
        lead,
        tags,
        parent_id: redact_opt(parent_id, |p| ObjectRef::Project(*p), in_scope),
        same_as: redact_opt(same_as, |s| ObjectRef::Federated(s), in_scope),
        target_date,
        progress_percent,
        details,
        color,
        image,
        archived,
        parts: parts
            .0
            .into_iter()
            .map(|p| guest_part(p, in_scope))
            .collect(),
        capabilities,
        form,
        deliverables: Deliverables(deliverables.0.into_iter().map(guest_deliverable).collect()),
        states,
        date_created,
        date_modified,
    }
}

/// A list of projects (`ProjectService::list`), each projected.
///
/// Projects outside the grant are not redacted to a marker — they are
/// simply not there (t[impl sharing.grant.absent]); a list is not a link.
pub fn guest_projects<S>(projects: Vec<ProjectInfo>, in_scope: &S) -> Vec<GuestProject>
where
    S: Fn(ObjectRef<'_>) -> bool + ?Sized,
{
    projects
        .into_iter()
        .filter(|p| in_scope(ObjectRef::Project(p.id)))
        .map(|p| guest_project(p, in_scope))
        .collect()
}

/// One part (`ProjectService::parts`, and nested in [`GuestProject`]).
pub fn guest_part<S>(part: Part, in_scope: &S) -> GuestPart
where
    S: Fn(ObjectRef<'_>) -> bool + ?Sized,
{
    let Part {
        id,
        name,
        references,
        components,
    } = part;
    GuestPart {
        id,
        name,
        references: redact_opt(references, |r| ObjectRef::Piece(*r), in_scope),
        components,
    }
}

/// Pieces — `ProjectService::pieces` and `ProjectService::setlist`.
///
/// A setlist assembles songs *by reference* from other projects
/// (`project.setlist.source`), so its entries are links: one outside the
/// grant leaks neither its id nor its name, and several collapse into
/// one marker.
pub fn guest_pieces<S>(pieces: Vec<Piece>, in_scope: &S) -> Vec<Linked<Piece>>
where
    S: Fn(ObjectRef<'_>) -> bool + ?Sized,
{
    let pieces = pieces.into_iter().map(|p| {
        let Piece { id, name, promoted } = p;
        Piece { id, name, promoted }
    });
    redact_list(pieces, |p| ObjectRef::Piece(p.id), in_scope)
}

/// One deliverable declaration. Carries no money and no link, so it
/// passes whole — but through a destructure, so a field added later is
/// decided here.
#[must_use]
pub fn guest_deliverable(d: Deliverable) -> Deliverable {
    let Deliverable {
        id,
        name,
        medium,
        scope,
        audience,
    } = d;
    Deliverable {
        id,
        name,
        medium,
        scope,
        audience,
    }
}

/// Expanded deliverable items (`deliverable_items`,
/// `client_deliverables`).
///
/// A per-part item's `title` is the piece's name, so an item over a
/// piece outside the grant is redacted whole, not just its `part` id.
/// Whole-project items (`part: None`) belong to the shared project.
pub fn guest_deliverable_items<S>(
    items: Vec<DeliverableItem>,
    in_scope: &S,
) -> Vec<Linked<DeliverableItem>>
where
    S: Fn(ObjectRef<'_>) -> bool + ?Sized,
{
    let mut out = Vec::new();
    let mut marked = false;
    for item in items {
        let DeliverableItem {
            deliverable,
            name,
            medium,
            audience,
            part,
            title,
        } = item;
        if part.is_none_or(|p| in_scope(ObjectRef::Piece(p))) {
            out.push(Linked::Shared(DeliverableItem {
                deliverable,
                name,
                medium,
                audience,
                part,
                title,
            }));
        } else if !marked {
            marked = true;
            out.push(Linked::NotShared);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use project_proto::{Audience, Capability, Deliverable, Medium, Parts, Scope};

    use super::*;
    use crate::link::{NOT_SHARED, ScopeSet};

    const CRESCENDUM: Uuid = Uuid::from_u128(0xC0E5_C0E5);
    const FTA_PARENT: Uuid = Uuid::from_u128(0xF7A0);
    const CLIENT: Uuid = Uuid::from_u128(0x00C1_1E47_0000_0000_0000_0000_0000_0001);
    const SONG: Uuid = Uuid::from_u128(0x5046);
    const OTHER_SONG: Uuid = Uuid::from_u128(0x07E4);

    /// Crescendum with every field set to something recognisable, so a
    /// value that survives the projection can be found in its output.
    fn crescendum() -> ProjectInfo {
        ProjectInfo {
            id: CRESCENDUM,
            path: "Projects/Crescendum.md".into(),
            title: "Crescendum".into(),
            status: "active".into(),
            priority: "high".into(),
            project_type: "general".into(),
            lead: "[[Cody]]".into(),
            tags: Tags(vec!["project".into()]),
            parts: Parts(vec![
                Part {
                    id: SONG,
                    name: "Overture".into(),
                    references: None,
                    components: vec![],
                },
                Part {
                    id: Uuid::from_u128(0x5E7),
                    name: "Encore".into(),
                    references: Some(OTHER_SONG),
                    components: vec![],
                },
            ]),
            capabilities: Capabilities::from_names([Capability::all()[0].as_str()]),
            form: None,
            deliverables: Deliverables(vec![Deliverable {
                id: Uuid::from_u128(0xDE1),
                name: "Album master".into(),
                medium: Medium::Audio,
                scope: Scope::WholeProject,
                audience: Audience::Client,
            }]),
            parent_id: Some(FTA_PARENT),
            same_as: Some("@other-org/secret-collab".into()),
            target_date: NaiveDate::from_ymd_opt(2026, 12, 1),
            progress_percent: 40,
            details: "The spring show.".into(),
            client_id: Some(CLIENT),
            billable_default: true,
            currency: "NZD".into(),
            default_rate_cents: 987_654_321,
            estimated_seconds: 123_456_789,
            agent_profile: "secret-agent-profile".into(),
            verify_command: "cargo test --secret-verify".into(),
            color: "#123456".into(),
            image: "https://example.test/cover.png".into(),
            archived: false,
            states: Some(project_proto::default_states()),
            date_created: Some(DateTime::<Utc>::MIN_UTC),
            date_modified: Some(DateTime::<Utc>::MIN_UTC),
        }
    }

    fn scope() -> ScopeSet {
        ScopeSet::new([CRESCENDUM, SONG, Uuid::from_u128(0x5E7)])
    }

    fn guest_json() -> serde_json::Value {
        let s = scope();
        serde_json::to_value(guest_project(crescendum(), &|r| s.contains(r))).unwrap()
    }

    fn guest_text() -> String {
        guest_json().to_string()
    }

    fn snake(camel: &str) -> String {
        let mut out = String::new();
        for c in camel.chars() {
            if c.is_ascii_uppercase() {
                out.push('_');
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    }

    /// One test per stripped field: the key is absent from the guest
    /// view (in either spelling) and the owner's value appears nowhere
    /// in it.
    macro_rules! stripped {
        ($($test:ident: $key:literal => $value:expr;)*) => {$(
            // t[verify sharing.grant.exclusions]
            #[test]
            fn $test() {
                let json = guest_json();
                let obj = json.as_object().unwrap();
                assert!(STRIPPED_PROJECT_FIELDS.contains(&$key));
                assert!(!obj.contains_key($key), "{} leaked", $key);
                assert!(!obj.contains_key(&snake($key)), "{} leaked", $key);
                let value: String = $value;
                assert!(
                    !guest_text().contains(&value),
                    "{}'s value {value} leaked",
                    $key
                );
            }
        )*};
    }

    stripped! {
        client_id_is_absent: "clientId" => CLIENT.to_string();
        billable_default_is_absent: "billableDefault" => "billable".to_owned();
        currency_is_absent: "currency" => "NZD".to_owned();
        default_rate_cents_is_absent: "defaultRateCents" => "987654321".to_owned();
        budget_estimated_seconds_is_absent: "estimatedSeconds" => "123456789".to_owned();
        agent_profile_is_absent: "agentProfile" => "secret-agent-profile".to_owned();
        verify_command_is_absent: "verifyCommand" => "secret-verify".to_owned();
    }

    /// The runtime half of the guard: every key `ProjectInfo` puts on
    /// the wire is either carried or stripped, by name. A new field
    /// that gets past the exhaustive destructure (by being added as
    /// `field: _`) still fails here until it is listed.
    #[test]
    fn every_project_field_is_classified_shared_or_stripped() {
        let full = serde_json::to_value(crescendum()).unwrap();
        let keys: BTreeSet<&str> = full
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let classified: BTreeSet<&str> = SHARED_PROJECT_FIELDS
            .iter()
            .chain(STRIPPED_PROJECT_FIELDS)
            .copied()
            .collect();
        assert_eq!(keys, classified, "a ProjectInfo field is unclassified");
        let guest = guest_json();
        let guest_keys: BTreeSet<&str> = guest
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for k in STRIPPED_PROJECT_FIELDS {
            assert!(!guest_keys.contains(k), "{k} leaked");
        }
    }

    // Leak-matrix row, unit level: "project RPC fields: get(Crescendum)
    // → client_id, default_rate_cents, budget absent". S10 re-checks it
    // end to end through the grant lane.
    // t[verify sharing.grant.exclusions]
    #[test]
    fn leak_matrix_get_crescendum_has_no_client_rate_or_budget() {
        let text = guest_text();
        for needle in [
            "clientId",
            "client_id",
            "defaultRateCents",
            "default_rate_cents",
            "estimatedSeconds",
            "budget",
            &CLIENT.to_string(),
            "987654321",
            "123456789",
        ] {
            assert!(!text.contains(needle), "{needle} in guest get(Crescendum)");
        }
        let g = guest_json();
        assert_eq!(g["title"], "Crescendum");
        assert_eq!(g["id"], CRESCENDUM.to_string());
    }

    // t[verify sharing.grant.absent]
    #[test]
    fn links_above_the_root_and_into_other_orgs_are_the_marker() {
        let g = guest_json();
        assert_eq!(g["parentId"], NOT_SHARED);
        assert_eq!(g["sameAs"], NOT_SHARED);
        let text = guest_text();
        assert!(!text.contains(&FTA_PARENT.to_string()));
        assert!(!text.contains("secret-collab"));
    }

    #[test]
    fn a_setlist_reference_outside_the_grant_is_redacted_and_in_scope_parts_pass() {
        let s = scope();
        let g = guest_project(crescendum(), &|r| s.contains(r));
        assert_eq!(g.parts.len(), 2);
        assert_eq!(g.parts[0].name, "Overture");
        assert_eq!(g.parts[0].references, None);
        assert_eq!(g.parts[1].references, Some(Linked::NotShared));
        assert!(!guest_text().contains(&OTHER_SONG.to_string()));
    }

    #[test]
    fn in_scope_links_pass_through() {
        let mut p = crescendum();
        p.parts.0[1].references = Some(SONG);
        let s = scope().with_paths(Vec::<String>::new());
        let mut s2 = s.clone();
        s2.ids.insert(FTA_PARENT);
        let g = guest_project(p, &|r| s2.contains(r));
        assert_eq!(g.parent_id, Some(Linked::Shared(FTA_PARENT)));
        assert_eq!(g.parts[1].references, Some(Linked::Shared(SONG)));
    }

    #[test]
    fn the_carried_fields_survive() {
        let s = scope();
        let g = guest_project(crescendum(), &|r| s.contains(r));
        let src = crescendum();
        assert_eq!(g.path, src.path);
        assert_eq!(g.details, src.details);
        assert_eq!(g.deliverables, src.deliverables);
        assert_eq!(g.states, src.states);
        assert_eq!(g.capabilities, src.capabilities);
        assert_eq!(g.target_date, src.target_date);
    }

    #[test]
    fn a_project_list_drops_projects_outside_the_grant() {
        let mut other = crescendum();
        other.id = Uuid::from_u128(0x07E5);
        other.title = "Another Client".into();
        let s = scope();
        let got = guest_projects(vec![crescendum(), other], &|r| s.contains(r));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, CRESCENDUM);
    }

    #[test]
    fn setlist_pieces_from_other_projects_leak_neither_id_nor_name() {
        let s = scope();
        let got = guest_pieces(
            vec![
                Piece {
                    id: SONG,
                    name: "Overture".into(),
                    promoted: false,
                },
                Piece {
                    id: OTHER_SONG,
                    name: "Someone Else's Hit".into(),
                    promoted: true,
                },
                Piece {
                    id: Uuid::from_u128(0x07E6),
                    name: "Another".into(),
                    promoted: true,
                },
            ],
            &|r| s.contains(r),
        );
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].shared().map(|p| p.name.as_str()), Some("Overture"));
        assert!(got[1].is_not_shared());
        let text = serde_json::to_string(&got).unwrap();
        assert!(!text.contains("Someone Else"));
        assert!(!text.contains(&OTHER_SONG.to_string()));
    }

    #[test]
    fn a_deliverable_item_over_an_unshared_piece_is_redacted_whole() {
        let s = scope();
        let item = |part: Option<Uuid>, title: &str| DeliverableItem {
            deliverable: Uuid::from_u128(0xDE1),
            name: "Per-song audio".into(),
            medium: Medium::Audio,
            audience: Audience::Client,
            part,
            title: title.into(),
        };
        let got = guest_deliverable_items(
            vec![
                item(None, "Album master"),
                item(Some(SONG), "Overture"),
                item(Some(OTHER_SONG), "Someone Else's Hit"),
            ],
            &|r| s.contains(r),
        );
        assert_eq!(got.len(), 3);
        assert!(got[2].is_not_shared());
        assert!(
            !serde_json::to_string(&got)
                .unwrap()
                .contains("Someone Else")
        );
    }
}
