//! What a new wiki page starts as, by type.
//!
//! "Add page" asks what kind of page it is and seeds the layout the house
//! style gives that kind (`skills/wiki-style.md`): the frontmatter it
//! needs, the opening it leads with, its sections, and its signature
//! element — with `%%hints%%` saying what goes where. Hints are editor
//! comments: visible while writing, gone when reading, and safe to leave.

/// A page type the picker offers: `(type, label, default folder)`.
pub const TYPES: &[(&str, &str, &str)] = &[
    ("topic", "Topic", "Topics"),
    ("question", "Question", "Questions"),
    ("passage", "Passage", "Passages"),
    ("person", "Person", "People"),
    ("word", "Word (Hebrew/Greek)", "Words"),
    ("source", "Source summary", "Sources"),
    ("path", "Study path", "Paths"),
];

/// The folder a page of `page_type` goes in when none was chosen.
#[must_use]
pub fn default_folder(page_type: &str) -> &'static str {
    TYPES
        .iter()
        .find(|(t, _, _)| *t == page_type)
        .map_or("", |(_, _, f)| f)
}

/// The new page's markdown. An unknown or empty type is a blank page
/// with just its title.
#[must_use]
pub fn template(page_type: &str, title: &str, today: &str) -> String {
    let quoted = title.replace('"', "\\\"");
    let (extra_fm, body) = match page_type {
        "topic" => (
            "tags: []\nsources: []\n",
            "%%The thesis in one paragraph: what this page claims.%%\n\n## First idea\n\n%%One idea per section. Cite each claim: [[source#^t0|0:00]]. Key texts as verse cards:%%\n\n[[bible::Book.C.V|Book C:V]] — what this verse adds\n\n> [!question]- How firm is this?\n> What is certain, what is not, and who disagrees.\n",
        ),
        "question" => (
            "tags: []\nsources: []\n",
            "## The short answer\n\n%%Two to four sentences. The answer, not a preview of it.%%\n\n## The case\n\n%%Claim by claim, each with its citation.%%\n\n```readings\nThe contested point?\n## First position\nWhat it says.\n+ Its best evidence\n- Its real weakness\nheld: Who holds it\n## Second position\nWhat it says.\n+ Its best evidence\n- Its real weakness\nheld: Who holds it\nverdict: Where this page lands\n```\n\n> [!question]- What this answer does not settle\n> The limits of the answer.\n",
        ),
        "passage" => (
            "tags: []\nanchors: []\nsources: []\n",
            "%%What the passage is, in a sentence.%%\n\n## Verse by verse\n\n| Verses | What happens |\n|---|---|\n| [[bible::Book.C.V\\|Book C:V]] | … |\n\n## How it has been read\n\n## Where it leads\n",
        ),
        "person" => (
            "tags: []\nsources: []\n",
            "%%Who they are, and why they matter here — one paragraph.%%\n\n## What they said or did\n\n%%With citations to where they appear.%%\n",
        ),
        "word" => (
            "tags: [word]\nlemma: \"\"\ntranslit: \"\"\nstrongs: \nlanguage: hebrew\ngloss: \"\"\n",
            "*%%The gloss, in a phrase.%%*\n\n%%Where it is used, with verse badges; how this wiki uses it. The word study (lexicon, occurrences) appears under the page from `strongs:`.%%\n",
        ),
        "source" => (
            "source_url: \"\"\ncontent_type: \nauthor: \"\"\nshort_title: \"\"\nsources: []\n",
            "%%Author · length · what it is.%%\n\n## Overview\n\n%%The argument in a paragraph or two.%%\n\n## Sections\n\n### First section [[source#^t0|0:00]]\n\n## Notes\n\n- [0:00] %%A note at a moment.%% ^t0-note1\n",
        ),
        "path" => (
            "tags: [path]\n",
            "%%Who this path is for, in a sentence. The app draws the progress and each page's footer.%%\n\n1. [[First page]] — why it is here\n2. [[Second page]] — why it is here\n",
        ),
        _ => ("", ""),
    };
    let type_line = if page_type.is_empty() {
        String::new()
    } else {
        format!("type: {page_type}\n")
    };
    let summary = if page_type.is_empty() {
        String::new()
    } else {
        "summary: \"\"\n".to_owned()
    };
    format!(
        "---\ntitle: \"{quoted}\"\n{type_line}{summary}{extra_fm}created: {today}\n---\n\n# {title}\n\n{body}"
    )
}

#[cfg(test)]
mod tests {
    use super::{TYPES, default_folder, template};

    #[test]
    fn every_type_seeds_its_layout() {
        for (t, _, folder) in TYPES {
            let md = template(t, "Title", "2026-09-28");
            assert!(md.starts_with("---\ntitle: \"Title\"\n"), "{t}: {md}");
            assert!(md.contains(&format!("type: {t}\n")), "{t}");
            assert!(md.contains("summary: \"\"\n"), "{t}");
            assert!(md.contains("\n# Title\n"), "{t}");
            assert_eq!(default_folder(t), *folder);
        }
        assert!(template("question", "Q?", "d").contains("```readings"));
        assert!(template("word", "W", "d").contains("strongs:"));
        assert_eq!(template("", "Plain", "d"), "---\ntitle: \"Plain\"\ncreated: d\n---\n\n# Plain\n\n");
    }
}
