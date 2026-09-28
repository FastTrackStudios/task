//! How a wiki page reads.
//!
//! A wiki page is long-form prose meant for someone else to read — a
//! study, an argument, an explanation — not a scratch note. So wiki pages
//! (scoped by the `.wiki-reading` class the page puts around the editor)
//! set in Literata, at a reading size and measure, with quieter citations:
//!
//! - **Type.** Literata, 17px / 1.65, optical sizing on. It carries the
//!   transliteration a study wiki needs — ḥērem, bĕnê hāʾĕlōhîm, nāšal —
//!   as real glyphs rather than a fallback font's.
//! - **Headings.** A title that is clearly the title, sections that are
//!   clearly sections, with more space above a heading than below it so
//!   it belongs to what follows.
//! - **Three kinds of reference, told apart.** A link to another page is
//!   the accent colour with a light underline; scripture is inline text
//!   with a dotted underline (no pill, so it never stretches the line); a
//!   timestamp into a video is a small muted `▶ 12:34` tag.
//! - **Tables.** Set smaller, in the UI face, with rule lines only
//!   between rows, and references that never break mid-reference.
//!
//! The font files are self-hosted (`assets/fonts/`, SIL Open Font
//! License — `OFL-Literata.txt` beside them), in two subsets the browser
//! fetches only when a page uses their characters.

use dioxus::prelude::*;

const LITERATA_LATIN: Asset = asset!("/assets/fonts/literata-latin-standard-normal.woff2");
const LITERATA_LATIN_ITALIC: Asset = asset!("/assets/fonts/literata-latin-standard-italic.woff2");
const LITERATA_LATIN_EXT: Asset = asset!("/assets/fonts/literata-latin-ext-standard-normal.woff2");
const LITERATA_LATIN_EXT_ITALIC: Asset =
    asset!("/assets/fonts/literata-latin-ext-standard-italic.woff2");

/// Fontsource's subset ranges for Literata.
const LATIN_RANGE: &str = "U+0000-00FF, U+0131, U+0152-0153, U+02BB-02BC, U+02C6, U+02DA, U+02DC, U+0304, U+0308, U+0329, U+2000-206F, U+20AC, U+2122, U+2191, U+2193, U+2212, U+2215, U+FEFF, U+FFFD";
const LATIN_EXT_RANGE: &str = "U+0100-02BA, U+02BD-02C5, U+02C7-02CC, U+02CE-02D7, U+02DD-02FF, U+0304, U+0308, U+0323, U+0329, U+1D00-1DBF, U+1E00-1E9F, U+1EF2-1EFF, U+2020, U+20A0-20AB, U+20AD-20C0, U+2113, U+2C60-2C7F, U+A720-A7FF";

/// The reading rules, after the `@font-face` block.
const READING_RULES: &str = r##"
/* The measure: about 68 characters of Literata at 17px. The strip above
   the title and the page body share it, so their edges line up. */
.wiki-reading .note-column { max-width: 44rem; }

.wiki-reading .editor-root {
    font-family: "Literata", Georgia, "Times New Roman", serif;
    font-size: 17px;
    line-height: 1.65;
    font-optical-sizing: auto;
    font-kerning: normal;
    text-rendering: optimizeLegibility;
    hanging-punctuation: first;
}
.wiki-reading .editor-root .md-bold { font-weight: 600; }

/* Vertical rhythm. Markdown separates paragraphs with a blank line, and
   the editor draws every blank line as a full empty row — a whole
   line-height of nothing between paragraphs, two of them (plus the
   heading's margin) above a heading. Here a blank line is a paragraph
   gap, not a line: about half a line, and next to nothing right after a
   heading, whose own margin already says where the section begins. A
   blank line is still a row you can click into and type on. */
.wiki-reading .editor-root .cm-line:has(> br:only-child) {
    min-height: 0;
    height: 0.55lh;
}
.wiki-reading .editor-root .cm-line.md-h1 + .cm-line:has(> br:only-child),
.wiki-reading .editor-root .cm-line.md-h2 + .cm-line:has(> br:only-child),
.wiki-reading .editor-root .cm-line.md-h3 + .cm-line:has(> br:only-child) {
    height: 0.2lh;
}

/* Headings: the title is the title; a section heading sits with the
   text under it (more room above than below). */
.wiki-reading .editor-root .cm-line.md-h1 {
    font-size: 1.95em;
    font-weight: 600;
    line-height: 1.2;
    letter-spacing: -0.012em;
    margin: 0 0 0.2em;
}
.wiki-reading .editor-root .cm-line.md-h2 {
    font-size: 1.32em;
    font-weight: 600;
    line-height: 1.3;
    letter-spacing: -0.006em;
    margin: 0.9em 0 0.1em;
}
.wiki-reading .editor-root .cm-line.md-h3 {
    font-size: 1.1em;
    font-weight: 600;
    line-height: 1.35;
    margin: 0.7em 0 0.05em;
}

/* Links to pages: the accent, a light underline that firms on hover. */
.wiki-reading .editor-root .md-wikilink {
    color: var(--ed-accent);
    text-decoration-line: underline;
    text-decoration-thickness: 1px;
    text-decoration-color: color-mix(in srgb, currentColor 35%, transparent);
    text-underline-offset: 3px;
}
.wiki-reading .editor-root .md-wikilink:hover {
    text-decoration-color: currentColor;
}

/* Timestamps into a video (`[[source#^t870|14:30]]`): a small muted tag,
   not a link-coloured word. */
.wiki-reading .editor-root .md-wikilink[data-href*="#^t"] {
    font-family: var(--font-sans, ui-sans-serif, system-ui, sans-serif);
    font-size: 0.7em;
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.01em;
    color: var(--ed-text-dim);
    text-decoration: none;
    border: 1px solid var(--ed-border);
    border-radius: 999px;
    padding: 0.05em 0.55em;
    vertical-align: 0.15em;
    white-space: nowrap;
}
.wiki-reading .editor-root .md-wikilink[data-href*="#^t"]::before {
    content: "\25B6\FE0E\00A0";
    font-size: 0.8em;
}
.wiki-reading .editor-root .md-wikilink[data-href*="#^t"]:hover {
    color: var(--ed-text);
    border-color: color-mix(in srgb, var(--ed-accent) 60%, transparent);
}

/* Scripture: inline, never a pill — a chip taller than the line spread
   every line it sat on. Dotted underline in the scripture colour; the
   verse is the tooltip. */
.wiki-reading .editor-root .md-scripture-chip {
    display: inline;
    padding: 0;
    border: 0;
    border-radius: 2px;
    background: none;
    color: var(--ed-text);
    font-weight: 500;
    white-space: nowrap;
    text-decoration-line: underline;
    text-decoration-style: dotted;
    text-decoration-thickness: 1.5px;
    text-decoration-color: color-mix(in srgb, var(--ed-tag) 80%, transparent);
    text-underline-offset: 3px;
}
.wiki-reading .editor-root .md-scripture-chip::before { content: none; }
.wiki-reading .editor-root .md-scripture-chip:hover {
    background: color-mix(in srgb, var(--ed-tag) 14%, transparent);
}

/* Tables: smaller, in the UI face, rules between rows only. */
.wiki-reading .editor-root .md-table {
    font-family: var(--font-sans, ui-sans-serif, system-ui, sans-serif);
    font-size: 0.8em;
    line-height: 1.45;
    margin: 0.35em 0 0.5em;
}
.wiki-reading .editor-root .md-table th,
.wiki-reading .editor-root .md-table td {
    border: 0;
    border-bottom: 1px solid var(--ed-border);
    padding: 0.55em 0.8em 0.55em 0;
    vertical-align: top;
}
.wiki-reading .editor-root .md-table th {
    background: none;
    color: var(--ed-text-dim);
    font-size: 0.85em;
    font-weight: 600;
    letter-spacing: 0.05em;
    text-transform: uppercase;
}
.wiki-reading .editor-root .md-table tr:nth-child(even) td { background: none; }
.wiki-reading .editor-root .md-table .md-scripture-chip { font-weight: 500; }

/* ── Phones ──────────────────────────────────────────────────
   A narrow screen gets a size that holds ~40 characters, headings that
   do not wrap to three lines, and tables that scroll sideways inside
   themselves rather than pushing the page wider than the screen. */
/* The phone's file-name strip repeats the tab; the action bar already
   says when there is something to save. */
.wiki-reading .note-mobile-status { display: none; }

@media (max-width: 640px) {
    .wiki-reading .editor-root { font-size: 16px; line-height: 1.6; }
    .wiki-reading .editor-root .cm-line.md-h1 { font-size: 1.65em; }
    .wiki-reading .editor-root .cm-line.md-h2 { font-size: 1.22em; margin-top: 0.8em; }
    .wiki-reading .editor-root .md-table {
        display: block;
        overflow-x: auto;
        -webkit-overflow-scrolling: touch;
        font-size: 0.85em;
    }
    .wiki-reading .editor-root .md-table td,
    .wiki-reading .editor-root .md-table th { min-width: 7.5rem; }
}

/* ── Touch ───────────────────────────────────────────────────
   A finger needs a bigger target than a pointer, and there is no hover:
   timestamp tags and scripture references grow a tap area without
   changing how the line looks. */
@media (pointer: coarse) {
    .wiki-reading .editor-root .md-wikilink[data-href*="#^t"] {
        padding: 0.25em 0.7em;
        font-size: 0.78em;
    }
    .wiki-reading .editor-root .md-scripture-chip {
        padding: 0.15em 0;
        margin: -0.15em 0;
    }
    .wiki-reading .editor-root .md-scripture-chip:hover { background: none; }
    /* The page's ⋯ and its menu rows: 44px, the touch minimum. */
    .wiki-reading .page-menu-button { min-width: 44px; min-height: 44px; margin: -12px -8px -12px auto; }
    .wiki-reading .page-menu button { min-height: 44px; }
    .wiki-reading .editor-root .md-wikilink:active,
    .wiki-reading .editor-root .md-scripture-chip:active {
        background: color-mix(in srgb, var(--ed-accent) 16%, transparent);
        border-radius: 3px;
    }
}

/* ── Large screens ───────────────────────────────────────────
   The measure stays the same — a wider column is harder to read, not
   easier — but on a big display the type grows with it. */
@media (min-width: 1800px) {
    .wiki-reading .editor-root { font-size: 18px; }
    .wiki-reading .note-column { max-width: 46rem; }
}
"##;

/// The stylesheet: Literata's faces, then the reading rules.
#[must_use]
pub fn reading_style() -> String {
    let face = |url: &Asset, style: &str, range: &str| {
        format!(
            "@font-face {{ font-family: \"Literata\"; font-style: {style}; font-display: swap; \
             font-weight: 200 900; src: url(\"{url}\") format(\"woff2\"); unicode-range: {range}; }}\n"
        )
    };
    let mut css = String::new();
    css.push_str(&face(&LITERATA_LATIN, "normal", LATIN_RANGE));
    css.push_str(&face(&LITERATA_LATIN_ITALIC, "italic", LATIN_RANGE));
    css.push_str(&face(&LITERATA_LATIN_EXT, "normal", LATIN_EXT_RANGE));
    css.push_str(&face(&LITERATA_LATIN_EXT_ITALIC, "italic", LATIN_EXT_RANGE));
    css.push_str(READING_RULES);
    css
}
