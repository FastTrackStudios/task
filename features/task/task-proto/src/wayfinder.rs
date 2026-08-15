//! The **wayfinder map** — a task whose body charts an effort too big
//! for one agent session.
//!
//! A map is not a new entity. It is an ordinary task with subtasks
//! (its workstreams), and the charting lives in the markdown body
//! under five known headings:
//!
//! - **Destination** — what reaching the end of this map looks like.
//!   Every session orients to it before choosing what to work.
//! - **Notes** — domain, skills to consult, standing preferences.
//! - **Decisions so far** — the index. One line per closed
//!   workstream: enough to judge relevance, then follow the link.
//! - **Not yet specified** — in-scope fog that cannot be stated
//!   sharply enough to become a workstream yet.
//! - **Out of scope** — work consciously ruled beyond the
//!   destination. Never graduates.
//!
//! The map is an **index, not a store**: a decision lives in exactly
//! one place — its workstream — and the map only gists it and links.
//!
//! # Why parsing, and not columns
//!
//! Decisions-so-far is append-only and read by humans in a diff. It
//! is the single worst thing to put in a database column, so the
//! markdown page stays the source of truth, matching every other
//! entity in the vault.
//!
//! # Fidelity
//!
//! Parsing is **lossless**. Anything before the first known heading,
//! and any heading this module does not know about, is preserved
//! verbatim and in its original position, so a page that has been
//! hand-edited round-trips unchanged. That matters because a map is
//! edited by hand at least as often as by an agent.

use crate::model::TaskInfo;

/// The five known sections, in the order a fresh map is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Section {
    Destination,
    Notes,
    DecisionsSoFar,
    NotYetSpecified,
    OutOfScope,
}

/// Every known section, in canonical order.
pub const SECTIONS: [Section; 5] = [
    Section::Destination,
    Section::Notes,
    Section::DecisionsSoFar,
    Section::NotYetSpecified,
    Section::OutOfScope,
];

impl Section {
    /// The heading text as written on the page.
    #[must_use]
    pub fn heading(self) -> &'static str {
        match self {
            Self::Destination => "Destination",
            Self::Notes => "Notes",
            Self::DecisionsSoFar => "Decisions so far",
            Self::NotYetSpecified => "Not yet specified",
            Self::OutOfScope => "Out of scope",
        }
    }

    /// Match a heading line's text. Case-insensitive, because these
    /// are written by hand as often as generated.
    #[must_use]
    pub fn parse(heading: &str) -> Option<Self> {
        let h = heading.trim().to_ascii_lowercase();
        SECTIONS
            .into_iter()
            .find(|s| s.heading().to_ascii_lowercase() == h)
    }
}

/// One chunk of a parsed map body, in page order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Chunk {
    /// Text before the first `## ` heading. Usually empty.
    Preamble(String),
    /// A `## ` section this module understands.
    Known { section: Section, body: String },
    /// Any other heading, kept verbatim so hand edits survive.
    Foreign { heading: String, body: String },
}

/// A parsed wayfinder map body.
///
/// Construct with [`MapBody::parse`], read sections with
/// [`MapBody::section`], and write back with [`MapBody::render`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MapBody {
    chunks: Vec<Chunk>,
}

impl MapBody {
    /// Parse a task body into sections, preserving everything.
    #[must_use]
    pub fn parse(details: &str) -> Self {
        let mut chunks: Vec<Chunk> = Vec::new();
        let mut preamble = String::new();
        let mut current: Option<(String, String)> = None;

        for line in details.lines() {
            if let Some(heading) = heading_of(line) {
                if let Some((h, body)) = current.take() {
                    chunks.push(finish(h, body));
                }
                current = Some((heading.to_string(), String::new()));
            } else if let Some((_, body)) = current.as_mut() {
                body.push_str(line);
                body.push('\n');
            } else {
                preamble.push_str(line);
                preamble.push('\n');
            }
        }
        if let Some((h, body)) = current.take() {
            chunks.push(finish(h, body));
        }
        if !preamble.trim().is_empty() {
            chunks.insert(0, Chunk::Preamble(preamble));
        }
        Self { chunks }
    }

    /// Read one section's body, trimmed.
    ///
    /// A section absent from the page reads as empty — a fresh map
    /// with only a Destination is a legitimate map, not an error.
    #[must_use]
    pub fn section(&self, section: Section) -> &str {
        self.chunks
            .iter()
            .find_map(|c| match c {
                Chunk::Known { section: s, body } if *s == section => Some(body.trim()),
                _ => None,
            })
            .unwrap_or("")
    }

    /// Is this section present on the page at all?
    ///
    /// Distinct from an empty body: a heading with nothing under it
    /// is a deliberate placeholder and must survive a round-trip.
    #[must_use]
    pub fn has_section(&self, section: Section) -> bool {
        self.chunks
            .iter()
            .any(|c| matches!(c, Chunk::Known { section: s, .. } if *s == section))
    }

    /// Replace a section's body, creating the section if absent.
    ///
    /// A newly created section is appended after the last known
    /// section that precedes it in canonical order, so a map built up
    /// piecemeal still reads in the documented order.
    pub fn set_section(&mut self, section: Section, body: &str) {
        let normalised = format!("\n{}\n", body.trim());
        if let Some(chunk) = self.chunks.iter_mut().find_map(|c| match c {
            Chunk::Known { section: s, body } if *s == section => Some(body),
            _ => None,
        }) {
            *chunk = normalised;
            return;
        }
        let at = self.insertion_point(section);
        self.chunks.insert(
            at,
            Chunk::Known {
                section,
                body: normalised,
            },
        );
    }

    /// Append one line to **Decisions so far**, the map's index.
    ///
    /// Append-only by construction: existing entries are never
    /// rewritten, because each one is the only pointer to a closed
    /// workstream's reasoning.
    pub fn append_decision(&mut self, line: &str) {
        let entry = {
            let l = line.trim();
            if l.starts_with("- ") {
                l.to_string()
            } else {
                format!("- {l}")
            }
        };
        let existing = self.section(Section::DecisionsSoFar).to_string();
        let combined = if existing.is_empty() {
            entry
        } else {
            format!("{existing}\n{entry}")
        };
        self.set_section(Section::DecisionsSoFar, &combined);
    }

    /// Every line currently in **Decisions so far**, in order.
    #[must_use]
    pub fn decisions(&self) -> Vec<&str> {
        self.section(Section::DecisionsSoFar)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect()
    }

    /// Render back to a markdown body.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        for chunk in &self.chunks {
            match chunk {
                Chunk::Preamble(text) => out.push_str(text),
                Chunk::Known { section, body } => {
                    push_section(&mut out, section.heading(), body);
                }
                Chunk::Foreign { heading, body } => push_section(&mut out, heading, body),
            }
        }
        out
    }

    /// Where a newly created section belongs, by canonical order.
    fn insertion_point(&self, section: Section) -> usize {
        let mut at = self.chunks.len();
        for (i, chunk) in self.chunks.iter().enumerate() {
            if let Chunk::Known { section: s, .. } = chunk {
                if *s > section {
                    at = i;
                    break;
                }
            }
        }
        at
    }
}

/// Read a map body off a task.
#[must_use]
pub fn map_body(t: &TaskInfo) -> MapBody {
    MapBody::parse(&t.details)
}

/// The `## ` heading text on this line, if it is one.
///
/// Only level-two headings delimit sections; a `### ` inside Notes is
/// content, not a new section.
fn heading_of(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("## ")?;
    if rest.starts_with('#') {
        return None;
    }
    Some(rest.trim())
}

fn finish(heading: String, body: String) -> Chunk {
    match Section::parse(&heading) {
        Some(section) => Chunk::Known { section, body },
        None => Chunk::Foreign { heading, body },
    }
}

fn push_section(out: &mut String, heading: &str, body: &str) {
    out.push_str("## ");
    out.push_str(heading);
    out.push('\n');
    out.push_str(body);
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = "\
## Destination

A working agent lane.

## Notes

Use the task skill.

## Decisions so far

- [Runner is a backend](t/1) — grown, not added
- [Ticket is the unit](t/2) — one worktree each

## Not yet specified

How cleanup is scheduled.

## Out of scope

Containers.
";

    #[test]
    fn all_five_sections_parse() {
        let m = MapBody::parse(FULL);
        assert_eq!(m.section(Section::Destination), "A working agent lane.");
        assert_eq!(m.section(Section::Notes), "Use the task skill.");
        assert_eq!(m.section(Section::NotYetSpecified), "How cleanup is scheduled.");
        assert_eq!(m.section(Section::OutOfScope), "Containers.");
        assert_eq!(m.decisions().len(), 2);
    }

    #[test]
    fn a_full_map_round_trips_byte_for_byte() {
        assert_eq!(MapBody::parse(FULL).render(), FULL);
    }

    #[test]
    fn sections_keep_their_order_through_a_round_trip() {
        let rendered = MapBody::parse(FULL).render();
        let order: Vec<_> = rendered
            .lines()
            .filter_map(heading_of)
            .map(str::to_string)
            .collect();
        assert_eq!(
            order,
            vec![
                "Destination",
                "Notes",
                "Decisions so far",
                "Not yet specified",
                "Out of scope"
            ]
        );
    }

    #[test]
    fn an_absent_section_reads_as_empty_rather_than_erroring() {
        let m = MapBody::parse("## Destination\n\nShip it.\n");
        assert_eq!(m.section(Section::OutOfScope), "");
        assert!(!m.has_section(Section::OutOfScope));
        assert_eq!(m.decisions(), Vec::<&str>::new());
    }

    #[test]
    fn an_empty_body_is_a_valid_empty_map() {
        let m = MapBody::parse("");
        for s in SECTIONS {
            assert_eq!(m.section(s), "", "{s:?} should be empty");
        }
        assert_eq!(m.render(), "");
    }

    #[test]
    fn appending_a_decision_preserves_every_existing_entry() {
        let mut m = MapBody::parse(FULL);
        m.append_decision("[Streaming is CRDT](t/3) — per run");
        let decisions = m.decisions();
        assert_eq!(decisions.len(), 3);
        assert_eq!(decisions[0], "- [Runner is a backend](t/1) — grown, not added");
        assert_eq!(decisions[1], "- [Ticket is the unit](t/2) — one worktree each");
        assert_eq!(decisions[2], "- [Streaming is CRDT](t/3) — per run");
    }

    #[test]
    fn appending_repeatedly_is_append_only() {
        let mut m = MapBody::parse(FULL);
        for i in 3..8 {
            m.append_decision(&format!("[d{i}](t/{i}) — gist"));
        }
        assert_eq!(m.decisions().len(), 7);
        // The originals are still first, and still intact.
        assert!(m.decisions()[0].contains("Runner is a backend"));
    }

    #[test]
    fn appending_to_a_map_without_the_section_creates_it_in_canonical_order() {
        let mut m = MapBody::parse("## Destination\n\nShip it.\n\n## Out of scope\n\nContainers.\n");
        m.append_decision("[First](t/1) — a start");
        let order: Vec<_> = m
            .render()
            .lines()
            .filter_map(heading_of)
            .map(str::to_string)
            .collect();
        assert_eq!(
            order,
            vec!["Destination", "Decisions so far", "Out of scope"],
            "a created section must land in documented order, not at the end"
        );
        assert_eq!(m.decisions(), vec!["- [First](t/1) — a start"]);
    }

    #[test]
    fn a_bare_decision_line_gains_its_bullet() {
        let mut m = MapBody::parse("");
        m.append_decision("[X](t/1) — y");
        assert_eq!(m.decisions(), vec!["- [X](t/1) — y"]);
        m.append_decision("- [Z](t/2) — w");
        assert_eq!(m.decisions()[1], "- [Z](t/2) — w", "an existing bullet is not doubled");
    }

    #[test]
    fn an_unknown_heading_survives_verbatim_and_in_place() {
        let src = "## Destination\n\nShip.\n\n## Prior art\n\nSee the other repo.\n\n## Notes\n\nn.\n";
        let m = MapBody::parse(src);
        assert_eq!(m.render(), src);
        assert_eq!(m.section(Section::Destination), "Ship.");
        assert_eq!(m.section(Section::Notes), "n.");
    }

    #[test]
    fn text_before_the_first_heading_is_preserved() {
        let src = "Some intro prose.\n\n## Destination\n\nShip.\n";
        assert_eq!(MapBody::parse(src).render(), src);
    }

    #[test]
    fn a_deeper_heading_inside_a_section_is_content_not_a_section() {
        let src = "## Notes\n\n### Sub\n\nbody\n";
        let m = MapBody::parse(src);
        assert!(m.section(Section::Notes).contains("### Sub"));
        assert_eq!(m.render(), src);
    }

    #[test]
    fn heading_matching_is_case_insensitive() {
        let m = MapBody::parse("## DECISIONS SO FAR\n\n- [a](t/1) — b\n");
        assert_eq!(m.decisions(), vec!["- [a](t/1) — b"]);
    }

    #[test]
    fn a_placeholder_heading_with_no_body_survives() {
        let src = "## Destination\n\nShip.\n\n## Out of scope\n";
        let m = MapBody::parse(src);
        assert!(m.has_section(Section::OutOfScope));
        assert_eq!(m.section(Section::OutOfScope), "");
        assert_eq!(m.render(), src);
    }

    #[test]
    fn setting_a_section_replaces_only_that_section() {
        let mut m = MapBody::parse(FULL);
        m.set_section(Section::Destination, "Something else entirely.");
        assert_eq!(m.section(Section::Destination), "Something else entirely.");
        assert_eq!(m.section(Section::Notes), "Use the task skill.");
        assert_eq!(m.decisions().len(), 2);
    }

    #[test]
    fn a_map_reads_off_a_task() {
        let mut t = TaskInfo::new("Agent lane");
        t.details = FULL.to_string();
        assert_eq!(map_body(&t).section(Section::Destination), "A working agent lane.");
    }
}
