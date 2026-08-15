//! In-note section tabs, packaged as a Task note **widget provider**.
//!
//! When a note's frontmatter opts in — `tabs: true` (any note) or the
//! legacy `type: event` — every top-level `# Section` heading AFTER the
//! title H1 renders as a tab (e.g. an event's Order | Teams | Times).
//!
//! Pure decoration layer: the tab bar renders as a widget where the first
//! section starts, and every non-active section (heading + body) is
//! hidden with `Replace` decorations. Tab clicks arrive as
//! `event-tab:<name>` hrefs through the editor's link channel; the active
//! tab lives in a global signal (one focused tabbed note at a time). The
//! caret escapes the system: if the selection sits inside a hidden
//! section, that section becomes active automatically, so editing never
//! fights the tabs.
//!
//! The shell knows none of this: [`widgets`] hands the decoration
//! pass + href handler to the `task-widgets` registry at the app root.

use dioxus::prelude::*;
use editor::state::EditorState;
use editor::{DecoratedRange, Decoration};
use task_widgets::{WidgetMatch, WidgetSpec};

/// The section-tab widget specs.
///
/// Two specs on purpose: the tab system itself claims both `type: event`
/// (legacy) and `tabs: true` notes, but header suppression is an
/// *event-type* behavior — the editor's typed title widget IS an event's
/// header — and must not leak onto arbitrary `tabs: true` notes.
#[must_use]
pub fn widgets() -> Vec<WidgetSpec> {
    vec![
        WidgetSpec::new(
            "note-tabs",
            vec![
                WidgetMatch::NoteType("event"),
                WidgetMatch::NoteFlag("tabs"),
            ],
        )
        .decorations(event_tab_decorations)
        .on_href(|href, _ctx| handle_tab_href(href))
        .plugin("core"),
        WidgetSpec::new(
            "note-tabs.event-header",
            vec![WidgetMatch::NoteType("event")],
        )
        .hide_note_header()
        .plugin("core"),
    ]
}

/// The active section tab (section name). Empty = first section.
pub static EVENT_ACTIVE_TAB: GlobalSignal<String> = Signal::global(String::new);

/// Handle a tab-selection href from the editor's link channel. Returns
/// `true` if it was a tab href and was applied (callers treat `true` as
/// "consumed").
pub fn handle_tab_href(href: &str) -> bool {
    if let Some(name) = href.strip_prefix("event-tab:") {
        *EVENT_ACTIVE_TAB.write() = name.to_string();
        return true;
    }
    false
}

/// Whether the note opts into section tabs: frontmatter `tabs: true`
/// (general) or `type: event` (legacy). Reads the leading `---` block.
fn wants_tabs(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("---") else {
        return false;
    };
    let Some((front, _)) = rest.split_once("\n---") else {
        return false;
    };
    front.lines().any(|l| {
        let l = l.trim_start();
        if let Some(v) = l.strip_prefix("type:") {
            if v.trim().trim_matches(['"', '\'']) == "event" {
                return true;
            }
        }
        if let Some(v) = l.strip_prefix("tabs:") {
            if v.trim().trim_matches(['"', '\'']) == "true" {
                return true;
            }
        }
        false
    })
}

/// Section spans: `(heading_line_start, body_end, name)` for every
/// top-level `#` after the first (the title).
fn sections(text: &str) -> Vec<(usize, usize, String)> {
    let mut heads: Vec<(usize, String)> = Vec::new();
    let mut pos = 0;
    let mut seen_title = false;
    let mut in_fence = false;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let t = content.trim_start();
        if t.starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence && content.starts_with("# ") {
            if seen_title {
                heads.push((pos, content[2..].trim().to_owned()));
            } else {
                seen_title = true;
            }
        }
        pos += line.len();
    }
    let mut out = Vec::new();
    for (i, (start, name)) in heads.iter().enumerate() {
        let end = heads.get(i + 1).map_or(text.len(), |(s, _)| *s);
        out.push((*start, end, name.clone()));
    }
    out
}

/// Decorations for the section-tab system (empty when the note hasn't
/// opted in or has fewer than two sections).
pub fn event_tab_decorations(state: &EditorState) -> Vec<DecoratedRange> {
    let text = state.doc.to_string();
    if !wants_tabs(&text) {
        return Vec::new();
    }
    let secs = sections(&text);
    if secs.len() < 2 {
        return Vec::new();
    }

    // Resolve the active section: caret-inside wins, then the signal,
    // then the first section.
    let caret = state.selection.primary().head;
    let caret_section = secs
        .iter()
        .find(|(s, e, _)| (*s..*e).contains(&caret))
        .map(|(_, _, n)| n.clone());
    let wanted = EVENT_ACTIVE_TAB.read().clone();
    let active = caret_section
        .or_else(|| {
            secs.iter()
                .find(|(_, _, n)| *n == wanted)
                .map(|(_, _, n)| n.clone())
        })
        .unwrap_or_else(|| secs[0].2.clone());

    let mut out = Vec::new();
    // Tab bar widget at the first section's start.
    let tabs: String = secs
        .iter()
        .map(|(_, _, name)| {
            let cls = if *name == active {
                "md-note-tab md-note-tab--active"
            } else {
                "md-note-tab"
            };
            format!(
                r#"<span class="{cls}" data-href="event-tab:{name}">{name}</span>"#,
                name = name
            )
        })
        .collect();
    out.push(Decoration::widget(
        secs[0].0,
        format!(r#"<span class="md-note-tabs">{tabs}</span>"#),
    ));
    for (start, end, name) in &secs {
        if *name == active {
            // Hide the active section's own `# Heading` line (the tab is
            // the label) — body stays.
            let head_end = text[*start..].find('\n').map_or(*end, |i| start + i + 1);
            out.push(Decoration::replace(*start..head_end));
        } else {
            out.push(Decoration::replace(*start..*end));
        }
    }
    out
}
