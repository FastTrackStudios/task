//! YAML frontmatter — parser, serializer, and properties widget
//! renderer. Sees only the document text and decoration system;
//! the live-preview entry point and the edit dispatcher (in
//! `editor-view`) call into this module via the public
//! [`parse_frontmatter`] / [`serialize_property`] helpers.
//!
//! Hand-rolled parser. Supports the shapes Obsidian itself
//! recognizes for the Properties panel:
//!   - `key: value`
//!   - `key:` followed by `  - item` lines (inline list)
//!   - `key: [a, b]` (flow list)
//!   - `key: true|false` (checkbox)
//!   - `tags: [...]` and other arrays
//!
//! Anything we don't understand falls back to a plain text row.
//! A real YAML lib (`saphyr` / `yaml-rust2`) can replace this
//! later — the byte-range-per-property contract is what the
//! edit path depends on, not the parser.

use super::escape_html;

#[derive(Debug, Clone)]
pub struct FrontMatter {
    /// Whole `---\n…\n---\n` span including delimiters.
    pub outer: std::ops::Range<usize>,
    pub opener: std::ops::Range<usize>,
    pub closer: std::ops::Range<usize>,
    pub props: Vec<Property>,
}

#[derive(Debug, Clone)]
pub struct Property {
    pub key: String,
    pub value: PropValue,
    /// Byte range of the whole property within the doc — from
    /// the start of the key line to the end of the last
    /// continuation line (inclusive of trailing newline). The
    /// editor uses this to replace the property on edit
    /// without touching the surrounding YAML.
    pub range: std::ops::Range<usize>,
}

#[derive(Debug, Clone)]
pub enum PropValue {
    Text(String),
    Bool(bool),
    Number(f64),
    Date(String), // YYYY-MM-DD; stored raw so we round-trip.
    List(Vec<String>),
    Empty,
}

impl PropValue {
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            PropValue::Text(_) => "text",
            PropValue::Bool(_) => "bool",
            PropValue::Number(_) => "number",
            PropValue::Date(_) => "date",
            PropValue::List(_) => "list",
            PropValue::Empty => "text",
        }
    }
}

#[must_use]
pub fn parse_frontmatter(text: &str) -> Option<FrontMatter> {
    let bytes = text.as_bytes();
    // Opening fence must be at position 0: literal `---` then
    // newline. Trailing whitespace on the line is tolerated.
    if !text.starts_with("---") {
        return None;
    }
    let opener_end = bytes.iter().position(|&b| b == b'\n')?;
    let first_line = &text[..opener_end];
    if first_line.trim_end() != "---" {
        return None;
    }
    let body_start = opener_end + 1;
    // Find the closing `---` line.
    let mut i = body_start;
    let mut closer_line_start = None;
    while i < bytes.len() {
        let line_from = i;
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        let line = &text[line_from..i];
        if line.trim_end() == "---" || line.trim_end() == "..." {
            closer_line_start = Some(line_from);
            break;
        }
        if i < bytes.len() {
            i += 1;
        }
    }
    let closer_start = closer_line_start?;
    let closer_end = bytes[closer_start..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(bytes.len(), |n| closer_start + n + 1);
    let props = parse_frontmatter_body(text, body_start, closer_start);
    Some(FrontMatter {
        outer: 0..closer_end,
        opener: 0..opener_end,
        closer: closer_start..(closer_end.saturating_sub(1)),
        props,
    })
}

/// Walk lines of the body between `[body_start, body_end)`,
/// emitting one `Property` per top-level `key:` line. Block
/// lists get merged into a single property whose `range` covers
/// all their `  - item` continuation lines, so an edit replaces
/// the whole multi-line entry atomically.
fn parse_frontmatter_body(text: &str, body_start: usize, body_end: usize) -> Vec<Property> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = body_start;
    while i < body_end {
        let line_from = i;
        while i < body_end && bytes[i] != b'\n' {
            i += 1;
        }
        let line_to = i;
        let line_with_nl = if i < body_end { i + 1 } else { i };
        let line = &text[line_from..line_to];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i = line_with_nl;
            continue;
        }
        if line.starts_with([' ', '\t']) {
            // Orphan continuation line — skip.
            i = line_with_nl;
            continue;
        }
        let Some(colon) = line.find(':') else {
            i = line_with_nl;
            continue;
        };
        let key = line[..colon].trim().to_string();
        if key.is_empty() {
            i = line_with_nl;
            continue;
        }
        let rest = line[colon + 1..].trim();
        let mut prop_end = line_with_nl;
        // YAML `|` literal block scalar: `key: |` followed by
        // indented lines preserves the line breaks as the value.
        // Strip the leading indent (whichever depth the first
        // line uses) from each continuation line.
        let value = if rest == "|" {
            let mut lines: Vec<String> = Vec::new();
            let mut indent: Option<usize> = None;
            let mut j = line_with_nl;
            while j < body_end {
                let sub_from = j;
                while j < body_end && bytes[j] != b'\n' {
                    j += 1;
                }
                let sub_line = &text[sub_from..j];
                let sub_nl = if j < body_end { j + 1 } else { j };
                let sub_indent = sub_line
                    .bytes()
                    .take_while(|&b| b == b' ' || b == b'\t')
                    .count();
                if sub_line.trim().is_empty() {
                    // Blank line: part of the block if any
                    // content follows at the right indent.
                    if indent.is_some() {
                        lines.push(String::new());
                        prop_end = sub_nl;
                    }
                    j = sub_nl;
                    continue;
                }
                let need = *indent.get_or_insert(sub_indent);
                if sub_indent < need {
                    break;
                }
                lines.push(sub_line[need..].to_string());
                prop_end = sub_nl;
                j = sub_nl;
            }
            i = prop_end;
            // Drop trailing blank lines so the value isn't padded
            // by whatever blank lines sat between the block and
            // the next key (or the closing `---`).
            while lines.last().is_some_and(std::string::String::is_empty) {
                lines.pop();
            }
            PropValue::Text(lines.join("\n"))
        } else if rest.is_empty() {
            // Block list: peek indented `- item` lines and pull
            // them into this property.
            let mut items: Vec<String> = Vec::new();
            let mut j = line_with_nl;
            while j < body_end {
                let sub_from = j;
                while j < body_end && bytes[j] != b'\n' {
                    j += 1;
                }
                let sub_line = &text[sub_from..j];
                let sub_nl = if j < body_end { j + 1 } else { j };
                if sub_line.starts_with([' ', '\t']) && sub_line.trim_start().starts_with("- ") {
                    let item = sub_line.trim_start()[2..]
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string();
                    items.push(item);
                    prop_end = sub_nl;
                    j = sub_nl;
                } else if sub_line.trim().is_empty() {
                    j = sub_nl;
                } else {
                    break;
                }
            }
            i = prop_end;
            if items.is_empty() {
                PropValue::Empty
            } else {
                PropValue::List(items)
            }
        } else {
            i = line_with_nl;
            classify_scalar(rest)
        };
        out.push(Property {
            key,
            value,
            range: line_from..prop_end,
        });
    }
    out
}

/// Classify a scalar YAML right-hand side into our enum.
/// Order matters: bools and flow lists win over the date /
/// number heuristics so `true` doesn't become a string.
fn classify_scalar(rest: &str) -> PropValue {
    if rest.starts_with('[') && rest.ends_with(']') {
        let inner = &rest[1..rest.len() - 1];
        let items: Vec<String> = inner
            .split(',')
            .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        return PropValue::List(items);
    }
    match rest {
        "true" | "True" | "TRUE" => return PropValue::Bool(true),
        "false" | "False" | "FALSE" => return PropValue::Bool(false),
        _ => {}
    }
    let cleaned = rest.trim_matches('"').trim_matches('\'').to_string();
    // YYYY-MM-DD heuristic — 10 chars, two dashes at fixed
    // positions, digits elsewhere.
    if cleaned.len() == 10
        && cleaned.as_bytes()[4] == b'-'
        && cleaned.as_bytes()[7] == b'-'
        && cleaned.bytes().enumerate().all(|(idx, b)| {
            if idx == 4 || idx == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
    {
        return PropValue::Date(cleaned);
    }
    // Number — parses as f64 and the original text contains no
    // letters. Rejects strings like "1.0.0".
    if !cleaned.is_empty()
        && cleaned
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E')
    {
        if let Ok(n) = cleaned.parse::<f64>() {
            return PropValue::Number(n);
        }
    }
    PropValue::Text(cleaned)
}

/// Render the parsed properties as an editable HTML widget.
/// Each value cell carries `data-prop-key` + `data-prop-from`/
/// `data-prop-to` so the JS-side handlers can dispatch typed
/// edit messages without re-parsing the YAML on every keystroke.
pub(crate) fn render_properties_html(props: &[Property], active_idx: Option<usize>) -> String {
    let mut html = String::from(r#"<div class="md-properties">"#);
    if props.is_empty() {
        html.push_str(r#"<div class="md-properties-empty">No properties</div>"#);
    }
    for (idx, p) in props.iter().enumerate() {
        let key_attr = escape_html(&p.key);
        let active_class = if Some(idx) == active_idx {
            " is-vim-active"
        } else {
            ""
        };
        html.push_str(&format!(
            r#"<div class="md-property-row{active_class}" data-prop-key="{key}" data-prop-from="{from}" data-prop-to="{to}" data-prop-type="{ty}">"#,
            key = key_attr,
            from = p.range.start,
            to = p.range.end,
            ty = p.value.type_name(),
        ));
        html.push_str(r#"<div class="md-property-key">"#);
        html.push_str(&key_attr);
        html.push_str("</div>");
        // Row gutter — × button shows on hover to remove the
        // whole property. Sits in the value column so the key
        // column stays tightly aligned.
        html.push_str(r#"<div class="md-property-value">"#);
        html.push_str(
            r#"<span class="md-property-row-remove" data-edit-role="row-remove" tabindex="0" title="Remove property">×</span>"#,
        );
        match &p.value {
            PropValue::Empty => {
                html.push_str(
                    r#"<span class="md-property-text" contenteditable="plaintext-only" spellcheck="false" data-edit-role="text"></span>"#,
                );
            }
            PropValue::Text(s) => {
                html.push_str(&format!(
                    r#"<span class="md-property-text" contenteditable="plaintext-only" spellcheck="false" data-edit-role="text">{}</span>"#,
                    escape_html(s),
                ));
            }
            PropValue::Number(n) => {
                let txt = format_number(*n);
                html.push_str(&format!(
                    r#"<span class="md-property-number" contenteditable="plaintext-only" spellcheck="false" data-edit-role="number" inputmode="decimal">{}</span>"#,
                    escape_html(&txt),
                ));
            }
            PropValue::Date(s) => {
                html.push_str(&format!(
                    r#"<input type="date" class="md-property-date" data-edit-role="date" value="{}"/>"#,
                    escape_html(s),
                ));
            }
            PropValue::Bool(b) => {
                html.push_str(&format!(
                    r#"<span class="md-property-bool" role="checkbox" tabindex="0" data-edit-role="bool" data-checked="{}" aria-checked="{}">{}</span>"#,
                    b,
                    b,
                    if *b { "✓" } else { "✗" },
                ));
            }
            PropValue::List(items) => {
                html.push_str(r#"<span class="md-property-list" data-edit-role="list">"#);
                for item in items {
                    html.push_str(&format!(
                        r#"<span class="md-property-chip" data-chip-value="{val}"><span class="md-chip-text">{val}</span><span class="md-chip-remove" data-edit-role="chip-remove" tabindex="0">×</span></span>"#,
                        val = escape_html(item),
                    ));
                }
                html.push_str(
                    r#"<span class="md-property-chip-add" contenteditable="plaintext-only" spellcheck="false" data-edit-role="chip-add" data-placeholder="add…"></span>"#,
                );
                html.push_str("</span>");
            }
        }
        html.push_str("</div></div>");
    }
    // Bottom "+ Add property" affordance. Clicking it focuses a
    // contenteditable key cell; Enter on a non-empty key
    // dispatches `prop-add`.
    html.push_str(
        r#"<div class="md-property-add-row"><span class="md-property-add" contenteditable="plaintext-only" spellcheck="false" data-edit-role="row-add" data-placeholder="+ Add property"></span></div>"#,
    );
    html.push_str("</div>");
    html
}

/// Format an `f64` so integers print without a decimal point
/// and floats keep enough precision to round-trip the YAML
/// source we parsed.
fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e16 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Re-serialize a property's YAML form. Returns a string that
/// includes its trailing newline so it can be dropped in for
/// `text[range]` as-is.
#[must_use]
pub fn serialize_property(key: &str, value: &PropValue) -> String {
    match value {
        PropValue::Text(s) if s.contains('\n') => {
            // Multi-line scalar — emit as a `|` block so the
            // newlines round-trip. Each line gets a two-space
            // indent, matching what the parser expects.
            let mut out = format!("{key}: |\n");
            for line in s.split('\n') {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
            out
        }
        PropValue::Text(s) => format!("{key}: {}\n", yaml_quote_if_needed(s)),
        PropValue::Bool(b) => format!("{key}: {b}\n"),
        PropValue::Number(n) => format!("{key}: {}\n", format_number(*n)),
        PropValue::Date(s) => format!("{key}: {s}\n"),
        PropValue::Empty => format!("{key}:\n"),
        PropValue::List(items) => {
            if items.is_empty() {
                format!("{key}: []\n")
            } else {
                let mut out = format!("{key}:\n");
                for item in items {
                    out.push_str("  - ");
                    out.push_str(&yaml_quote_if_needed(item));
                    out.push('\n');
                }
                out
            }
        }
    }
}

/// YAML scalar quoting. Wraps the string in double quotes only
/// when needed — values containing colons, leading dashes, or
/// reserved keywords would otherwise re-parse as something
/// else. Newlines aren't allowed at all in single-line scalars
/// so they're replaced with spaces.
fn yaml_quote_if_needed(s: &str) -> String {
    let needs_quote = s.is_empty()
        || s.starts_with(' ')
        || s.ends_with(' ')
        || s.contains(':')
        || s.contains('#')
        || s.contains('[')
        || s.contains(']')
        || s.contains('{')
        || s.contains('}')
        || s.contains(',')
        || matches!(
            s,
            "true" | "false" | "True" | "False" | "TRUE" | "FALSE" | "null" | "~"
        )
        || s.starts_with('-')
        || s.starts_with('?')
        || s.starts_with('|')
        || s.starts_with('>')
        || s.starts_with('&')
        || s.starts_with('*')
        || s.starts_with('!')
        || s.starts_with('%')
        || s.starts_with('@')
        || s.starts_with('`');
    let cleaned = s.replace('\n', " ").replace('\r', "");
    if needs_quote {
        format!("\"{}\"", cleaned.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        cleaned
    }
}
