//! Persisted calendar event — what the user drops onto the schedule.
//!
//! Distinct from [`crate::EventType`] (a cal.com-style bookable
//! template) and [`crate::DayPlan`] blocks (the day's structure): a
//! `CalEvent` is one concrete event on the timeline. The
//! `view-calendar` UI type maps onto this for storage. Times are
//! RFC-3339 strings and `color` is the `ColorTag` name so the proto
//! stays serializer-agnostic and `Facet`-encodable.
//!
//! Beyond its time, an event can carry:
//!
//! - [`EventLink`]s — typed links to vault notes (`for` →
//!   `[[Gigs/caa-2026]]`). On disk each relation is one frontmatter
//!   property whose value is a list of wikilinks, so the vault's link
//!   index counts the event as a backlink of the note and a `.base` can
//!   filter on the property (`note.for`) — see
//!   `features/scheduling/spec/scheduling.md`.
//! - an [`ExternalRef`] — the id an integration (Planning Center, a
//!   CalDAV peer) knows the event by, so a re-sync finds the event it
//!   made last time instead of making a second one.
//! - a `tombstone` — the stamp an integration sets when the event was
//!   removed upstream. The event stays on disk (with its links and its
//!   history); listings hide it unless asked.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// One typed link from an event to a vault note.
///
/// The vault-property form of a typed link: `relation` is the
/// frontmatter key (`for`, `rehearsal-for`, `venue`), `target` the
/// wikilink, always stored bracketed (`[[Gigs/caa-2026]]`).
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct EventLink {
    /// Lowercase slug (`[a-z][a-z0-9_-]*`), never one of
    /// [`CalEvent::RESERVED_KEYS`].
    pub relation: String,
    /// The linked note as a wikilink: `[[Gigs/caa-2026]]`.
    pub target: String,
}

impl EventLink {
    /// A link with `target` normalized to wikilink form: a bare
    /// `Gigs/caa-2026` (or `Gigs/caa-2026.md`) becomes
    /// `[[Gigs/caa-2026]]`; an already-bracketed target is kept.
    #[must_use]
    pub fn new(relation: impl Into<String>, target: &str) -> Self {
        Self {
            relation: relation.into(),
            target: wikilink(target),
        }
    }

    /// The target without its brackets (and without any `|alias`):
    /// `Gigs/caa-2026`.
    #[must_use]
    pub fn target_path(&self) -> &str {
        let inner = self
            .target
            .trim()
            .trim_start_matches("[[")
            .trim_end_matches("]]");
        inner.split('|').next().unwrap_or(inner).trim()
    }

    /// Does this link point at `note`? `note` may be bracketed or
    /// bare, with or without `.md`, and match by full path or by
    /// basename (wikilinks resolve by basename too).
    #[must_use]
    pub fn points_at(&self, note: &str) -> bool {
        let want = strip_md(Self::new("", note).target_path()).to_owned();
        let have = strip_md(self.target_path());
        have == want
            || have.rsplit('/').next() == Some(want.as_str())
            || want.rsplit('/').next() == Some(have)
    }

    /// Is `relation` usable as a frontmatter key for a link?
    #[must_use]
    pub fn valid_relation(relation: &str) -> bool {
        let mut chars = relation.chars();
        chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            && !CalEvent::RESERVED_KEYS.contains(&relation)
    }
}

fn strip_md(s: &str) -> &str {
    s.strip_suffix(".md").unwrap_or(s)
}

/// `target` in `[[…]]` form.
fn wikilink(target: &str) -> String {
    let t = target.trim();
    if t.starts_with("[[") && t.ends_with("]]") {
        t.to_owned()
    } else {
        format!("[[{}]]", strip_md(t))
    }
}

/// `Vec<EventLink>` newtype — a JSON column on [`CalEvent`].
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct EventLinks(pub Vec<EventLink>);

impl std::ops::Deref for EventLinks {
    type Target = Vec<EventLink>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for EventLinks {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<Vec<EventLink>> for EventLinks {
    fn from(v: Vec<EventLink>) -> Self {
        Self(v)
    }
}

impl FromIterator<EventLink> for EventLinks {
    fn from_iter<I: IntoIterator<Item = EventLink>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// Where an event came from outside Task: `{system: "pco", id:
/// "plan_time:123"}`. Written as one string, `system:id`
/// (`pco:plan_time:123`) — the system is everything before the first
/// `:`, so ids may carry colons of their own.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, PartialEq, Eq, Hash, Facet, Serialize, Deserialize,
)]
pub struct ExternalRef {
    /// The integration (`pco`, `caldav`, `google`). Lowercase, no `:`.
    pub system: String,
    /// The upstream id, opaque to Task.
    pub id: String,
}

impl ExternalRef {
    /// Parse `system:id`. `None` when either half is empty.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let (system, id) = s.trim().split_once(':')?;
        let (system, id) = (system.trim(), id.trim());
        (!system.is_empty() && !id.is_empty()).then(|| Self {
            system: system.to_owned(),
            id: id.to_owned(),
        })
    }
}

impl std::fmt::Display for ExternalRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.system, self.id)
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "scheduling_cal_events", repo)]
pub struct CalEvent {
    /// Stable id (uuid string). PK — a new event's vault file is
    /// `<id>.md`; an existing file keeps its name.
    #[architect(primary_key, auto_increment = false)]
    pub id: String,
    #[architect(filterable, sortable, fulltext)]
    pub title: String,
    /// Inclusive start, RFC-3339.
    #[architect(filterable, sortable)]
    pub start: String,
    /// Exclusive end, RFC-3339.
    #[architect(filterable, sortable)]
    pub end: String,
    #[architect(filterable)]
    pub all_day: bool,
    /// `ColorTag` name (`"primary"`, `"success"`, …).
    #[architect(filterable)]
    pub color: String,
    #[architect(fulltext)]
    pub description: Option<String>,
    /// RFC-5545 RRULE, if recurring.
    pub recurrence: Option<String>,
    /// Where it happens — free text (`The Hall, 12 Main St`).
    #[architect(fulltext)]
    #[serde(default)]
    pub location: Option<String>,
    /// Typed links to vault notes, in the order they were given.
    #[architect(json)]
    #[serde(default)]
    pub links: EventLinks,
    /// The id an integration knows this event by.
    #[architect(json, filterable)]
    #[serde(default)]
    pub external_ref: Option<ExternalRef>,
    /// RFC-3339 stamp of when the event was found removed upstream.
    /// `None` = live.
    #[architect(filterable)]
    #[serde(default)]
    pub tombstone: Option<String>,
}

impl CalEvent {
    /// Frontmatter keys the event itself owns. Anything else on an
    /// event file is either a link relation (its value is wikilinks)
    /// or a property somebody else set, which a rewrite keeps.
    pub const RESERVED_KEYS: &'static [&'static str] = &[
        "type",
        "id",
        "title",
        "start",
        "end",
        "all_day",
        "color",
        "description",
        "recurrence",
        "location",
        "external_ref",
        "tombstone",
        "tags",
        "aliases",
        "cssclasses",
    ];

    /// An event with nothing but its required fields set.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        start: impl Into<String>,
        end: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            start: start.into(),
            end: end.into(),
            all_day: false,
            color: "primary".into(),
            description: None,
            recurrence: None,
            location: None,
            links: EventLinks::default(),
            external_ref: None,
            tombstone: None,
        }
    }

    /// Does any of this event's links point at `note`?
    #[must_use]
    pub fn links_to(&self, note: &str) -> bool {
        self.links.iter().any(|l| l.points_at(note))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_ref_round_trips_through_its_string() {
        let r = ExternalRef::parse("pco:plan_time:123").expect("parse");
        assert_eq!(r.system, "pco");
        assert_eq!(r.id, "plan_time:123");
        assert_eq!(r.to_string(), "pco:plan_time:123");
        assert!(ExternalRef::parse("pco:").is_none());
        assert!(ExternalRef::parse("no-colon").is_none());
    }

    #[test]
    fn link_targets_normalize_to_wikilinks() {
        assert_eq!(EventLink::new("for", "Gigs/caa-2026").target, "[[Gigs/caa-2026]]");
        assert_eq!(EventLink::new("for", "Gigs/caa-2026.md").target, "[[Gigs/caa-2026]]");
        assert_eq!(EventLink::new("for", "[[Gigs/caa-2026]]").target, "[[Gigs/caa-2026]]");
        let l = EventLink::new("for", "[[Gigs/caa-2026|CAA]]");
        assert_eq!(l.target_path(), "Gigs/caa-2026");
        assert!(l.points_at("Gigs/caa-2026.md"));
        assert!(l.points_at("caa-2026"));
        assert!(!l.points_at("Gigs/other"));
    }

    #[test]
    fn relations_are_slugs_that_are_not_event_keys() {
        assert!(EventLink::valid_relation("for"));
        assert!(EventLink::valid_relation("rehearsal-for"));
        assert!(!EventLink::valid_relation("title"));
        assert!(!EventLink::valid_relation("For"));
        assert!(!EventLink::valid_relation(""));
        assert!(!EventLink::valid_relation("a b"));
    }
}
