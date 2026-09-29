//! The house style, checked where a program can check it.
//!
//! `skills/wiki-style.md` is the style guide an agent (or a person)
//! writes to. Most of it is judgment; some of it is not — a page either
//! has a `summary:` or it doesn't. This finds those, page by page, so a
//! wiki can list what needs tidying the way it lists what needs a second
//! voice.

use crate::parse::Page;

/// The most a `summary:` should say — it is a lede and a hover card.
const SUMMARY_MAX: usize = 140;

/// Headings that hold doubt, which the style folds rather than leaves
/// open at the end of the argument.
const DOUBT_HEADINGS: &[&str] = &[
    "contested",
    "how firm",
    "how strong",
    "other readings",
    "what this answer does not settle",
    "caveats",
];

/// t[impl wiki.gaps.style]
/// What the style check says about one page, in reading order. Empty
/// when there is nothing to say.
pub(crate) fn style_notes(page: &Page) -> Vec<String> {
    let mut notes = Vec::new();
    let body = page.body.as_str();

    if page.summary.is_empty() {
        notes.push("no summary: — add one sentence for the lede and hover card".to_owned());
    } else if page.summary.chars().count() > SUMMARY_MAX {
        notes.push(format!("the summary is over {SUMMARY_MAX} characters"));
    }

    let h1s = body.lines().filter(|l| l.starts_with("# ")).count();
    if h1s > 1 {
        notes.push("more than one # title".to_owned());
    }
    let first_para = body
        .lines()
        .skip_while(|l| !l.starts_with("# "))
        .skip(1)
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim();
    if !page.summary.is_empty() && first_para == page.summary {
        notes.push("the first paragraph repeats the summary".to_owned());
    }

    let mut in_fence = false;
    let mut bold_year_bullets = 0;
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("```") {
            let lang = t.trim_start_matches('`').trim();
            if !in_fence && (lang == "mermaid" || lang == "typst" || lang == "math") {
                notes.push(format!("a ```{lang} block — not rendered in Task"));
            }
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(h) = t.strip_prefix("## ") {
            let h = h.trim().to_lowercase();
            if DOUBT_HEADINGS.iter().any(|d| h.starts_with(d)) {
                notes.push(format!(
                    "“## {}” is open — fold it: > [!question]- {}",
                    h.trim(),
                    h.trim()
                ));
            }
        }
        if t.starts_with("- **") && t[4..].starts_with(|c: char| c.is_ascii_digit()) {
            bold_year_bullets += 1;
        }
        if shows_bible_code(line) {
            notes.push(
                "a bible:: code shows as text — make it a [[bible::…|Full Name C:V]] link"
                    .to_owned(),
            );
        }
        if page.page_type != "source" && bare_timestamp(line) {
            notes.push("a bare timestamp — cite it: [[source#^t<seconds>|mm:ss]]".to_owned());
        }
    }
    if bold_year_bullets >= 2 {
        notes.push("bold-year bullets — a ```timeline block".to_owned());
    }
    notes.dedup();
    notes
}

/// `bible::` outside any `[[…]]` on the line.
fn shows_bible_code(line: &str) -> bool {
    outside_links(line).contains("bible::")
}

/// A `(12:34)` or `[1:02:03]` that is not inside a link.
fn bare_timestamp(line: &str) -> bool {
    let text = outside_links(line);
    let b = text.as_bytes();
    b.iter().enumerate().any(|(i, &open)| {
        (open == b'(' || open == b'[') && {
            let rest = &text[i + 1..];
            let stamp: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == ':')
                .collect();
            let close = rest[stamp.len()..].chars().next();
            stamp.contains(':')
                && stamp.split(':').all(|p| !p.is_empty())
                && matches!(close, Some(')' | ']'))
        }
    })
}

/// The line with every `[[…]]` taken out.
fn outside_links(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find("[[") {
        out.push_str(&rest[..i]);
        match rest[i..].find("]]") {
            Some(j) => rest = &rest[i + j + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::style_notes;
    use crate::parse::parse_page;

    fn notes(md: &str) -> Vec<String> {
        style_notes(&parse_page("Topics/T.md".to_owned(), md))
    }

    #[test]
    fn a_page_in_the_house_style_has_nothing_to_say() {
        let md = "---\ntitle: T\ntype: topic\nsummary: \"One line.\"\n---\n\n# T\n\nThe claim [[s#^t12|0:12]], and [[bible::Ps.82.1|Psalm 82:1]].\n\n> [!question]- How firm is this?\n> Fairly.\n";
        assert!(notes(md).is_empty(), "{:?}", notes(md));
    }

    #[test]
    fn what_the_check_finds() {
        let md = "---\ntitle: T\ntype: topic\n---\n\n# T\n\nText bible::Ps.82.1 and a stamp (12:34).\n\n- **1928** — a\n- **1929** — b\n\n```mermaid\ngraph\n```\n\n## Contested\n\nMaybe.\n";
        let n = notes(md).join(" | ");
        for want in [
            "no summary",
            "bible::",
            "bare timestamp",
            "timeline",
            "mermaid",
            "“## contested” is open",
        ] {
            assert!(n.contains(want), "missing {want}: {n}");
        }
    }

    #[test]
    fn a_summary_repeated_as_the_opening_is_noted() {
        let md = "---\ntitle: T\nsummary: \"Same.\"\n---\n\n# T\n\nSame.\n";
        assert!(notes(md).iter().any(|n| n.contains("repeats")));
    }
}
