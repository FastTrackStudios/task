//! System prompt for the inbox daily processing pass.
//!
//! Follows `agent-wiki::prompts`' template discipline: a
//! `const` template with `{name}` placeholders filled by
//! [`render`], and a strict fenced-block output contract the
//! sibling [`crate::parsers`] module consumes.

use std::collections::HashMap;

/// System prompt for the batch triage turn. Placeholders:
/// `{today}` (ISO date), `{projects}` (one `- <title>` line
/// per project, or a "(no projects)" sentinel), and
/// `{inbox_items}` (the block built by
/// [`crate::bridge::render_items_block`]).
pub const PROCESS_SYSTEM: &str = include_str!("templates/process_system.txt");

/// Substitute `{key}` placeholders in `template` with values
/// from `vars`. Keys not present in `vars` are left as
/// literal `{key}` (same intentional behavior as
/// `agent_wiki::prompts::render` — missing context stays
/// visible in the prompt rather than silently vanishing).
#[must_use]
pub fn render<S: std::hash::BuildHasher>(template: &str, vars: &HashMap<&str, &str, S>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('}') {
            let key = &after[..end];
            if let Some(val) = vars.get(key) {
                out.push_str(val);
            } else {
                out.push('{');
                out.push_str(key);
                out.push('}');
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[start..]);
            break;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_known_keys() {
        let mut vars = HashMap::new();
        vars.insert("today", "2026-07-03");
        assert_eq!(render("Date: {today}.", &vars), "Date: 2026-07-03.");
    }

    #[test]
    fn render_leaves_unknown_keys_literal() {
        let vars = HashMap::new();
        assert_eq!(render("Hi {nobody}", &vars), "Hi {nobody}");
    }

    #[test]
    fn process_system_renders_fully() {
        let mut vars = HashMap::new();
        vars.insert("today", "2026-07-03");
        vars.insert("projects", "- Groceries");
        vars.insert("inbox_items", "- id: abc\n  body: buy milk");
        let out = render(PROCESS_SYSTEM, &vars);
        assert!(out.contains("Today's date: 2026-07-03"));
        assert!(out.contains("- Groceries"));
        assert!(out.contains("buy milk"));
        // Every placeholder the template declares must be filled.
        for key in ["{today}", "{projects}", "{inbox_items}"] {
            assert!(!out.contains(key), "unfilled placeholder {key}");
        }
    }
}
