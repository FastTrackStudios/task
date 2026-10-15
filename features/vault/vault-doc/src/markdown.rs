//! Markdown ⇄ document shape: a vault file is YAML frontmatter
//! (`props`) plus a markdown body.
//!
//! Two directions, both pure:
//!
//! - [`parse`] reads a file into its [`Parsed`] parts — the frontmatter's
//!   top-level keys as Loro values, and the body text;
//! - [`materialise`] writes props + body back as a file. It takes the
//!   **previous** file text when there is one and keeps every
//!   frontmatter entry whose value did not change *byte for byte as it
//!   was* (comments, quoting, key order), so a document round-tripped
//!   through the app never rewrites lines nobody touched — an Obsidian
//!   note keeps its diff clean, and a `jj` history stays readable.
//!
//! The body is everything after the close fence, exactly (including the
//! blank line a task file sets its description off with — see
//! [`crate::display`] for the convention an editor shows). With no
//! frontmatter the whole file is body.

use std::collections::BTreeMap;

use crdt::loro::LoroValue;

/// A document's properties: frontmatter keys → values. Ordered so
/// materialising new keys is deterministic.
pub type Props = BTreeMap<String, LoroValue>;

/// A file read into document shape.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed<'a> {
    /// What the frontmatter says, when it is there.
    pub front: Front,
    /// The body: everything after the frontmatter's close fence.
    pub body: &'a str,
}

/// The state of a file's frontmatter.
#[derive(Debug, Clone, PartialEq)]
pub enum Front {
    /// No frontmatter block — the whole file is body.
    None,
    /// A block whose YAML is a mapping (or empty): its keys.
    Props(Props),
    /// A block whose YAML does not parse as a mapping (half typed, or
    /// hand-broken). Read as **no frontmatter**: the whole text is body,
    /// so nothing anyone wrote is lost — the props come back when the
    /// YAML parses again. An editor mid-keystroke should not push such a
    /// text at all ([`is_mid_edit`]).
    Invalid,
}

impl Parsed<'_> {
    /// The props this file states (none, for [`Front::None`] and
    /// [`Front::Invalid`]).
    #[must_use]
    pub fn props(&self) -> Option<Props> {
        match &self.front {
            Front::None | Front::Invalid => Some(Props::new()),
            Front::Props(p) => Some(p.clone()),
        }
    }
}

/// A text whose frontmatter is half written — a block that does not
/// parse, or an open fence with no close. An editor holds such a buffer
/// back from its document until it parses, so a key being typed is
/// never read as "every property deleted".
#[must_use]
pub fn is_mid_edit(text: &str) -> bool {
    match fences(text) {
        Some(_) => matches!(parse(text).front, Front::Invalid),
        None => text.starts_with("---\n") && !text[4..].contains("\n---"),
    }
}

/// Byte ranges of a file's frontmatter: the YAML between the fences, and
/// where the body starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fences {
    /// The YAML text between the fences (no fence lines).
    pub yaml: (usize, usize),
    /// The first byte of the body.
    pub body_start: usize,
}

/// Find a leading frontmatter block: `---\n`, YAML, then `\n---\n` (or a
/// `\n---` that ends the file). An empty block (`---\n---\n`) counts.
#[must_use]
pub fn fences(text: &str) -> Option<Fences> {
    let rest = text.strip_prefix("---\n")?;
    // An empty block: the close fence follows the open one directly.
    if rest.starts_with("---\n") {
        return Some(Fences {
            yaml: (4, 4),
            body_start: 8,
        });
    }
    if rest == "---" {
        return Some(Fences {
            yaml: (4, 4),
            body_start: text.len(),
        });
    }
    if let Some(i) = rest.find("\n---\n") {
        return Some(Fences {
            yaml: (4, 4 + i),
            body_start: 4 + i + 5,
        });
    }
    rest.ends_with("\n---").then(|| Fences {
        yaml: (4, text.len() - 4),
        body_start: text.len(),
    })
}

/// Read a file into document shape.
#[must_use]
pub fn parse(text: &str) -> Parsed<'_> {
    let Some(f) = fences(text) else {
        return Parsed {
            front: Front::None,
            body: text,
        };
    };
    let yaml = &text[f.yaml.0..f.yaml.1];
    match parse_yaml_props(yaml) {
        Some(p) => Parsed {
            front: Front::Props(p),
            body: &text[f.body_start..],
        },
        None => Parsed {
            front: Front::Invalid,
            body: text,
        },
    }
}

fn parse_yaml_props(yaml: &str) -> Option<Props> {
    if yaml.trim().is_empty() {
        return Some(Props::new());
    }
    match serde_yaml::from_str::<serde_yaml::Value>(yaml).ok()? {
        serde_yaml::Value::Mapping(m) => Some(
            m.into_iter()
                .map(|(k, v)| (key_string(&k), yaml_to_loro(v)))
                .collect(),
        ),
        serde_yaml::Value::Null => Some(Props::new()),
        _ => None,
    }
}

fn key_string(k: &serde_yaml::Value) -> String {
    match k {
        serde_yaml::Value::String(s) => s.clone(),
        other => serde_yaml::to_string(other)
            .map(|s| s.trim_end().to_owned())
            .unwrap_or_default(),
    }
}

/// YAML → Loro value. Nested mappings and sequences become Loro map and
/// list *values* (the property merges as one unit, by key).
#[must_use]
pub fn yaml_to_loro(v: serde_yaml::Value) -> LoroValue {
    match v {
        serde_yaml::Value::Null => LoroValue::Null,
        serde_yaml::Value::Bool(b) => LoroValue::Bool(b),
        serde_yaml::Value::Number(n) => n.as_i64().map_or_else(
            || LoroValue::Double(n.as_f64().unwrap_or_default()),
            LoroValue::I64,
        ),
        serde_yaml::Value::String(s) => LoroValue::from(s),
        serde_yaml::Value::Sequence(seq) => {
            LoroValue::List(seq.into_iter().map(yaml_to_loro).collect())
        }
        serde_yaml::Value::Mapping(m) => LoroValue::Map(
            m.into_iter()
                .map(|(k, v)| (key_string(&k), yaml_to_loro(v)))
                .collect(),
        ),
        serde_yaml::Value::Tagged(t) => yaml_to_loro(t.value),
    }
}

/// Loro value → YAML. Map keys sort, so a value materialises the same
/// way on every device.
#[must_use]
pub fn loro_to_yaml(v: &LoroValue) -> serde_yaml::Value {
    match v {
        LoroValue::Null | LoroValue::Container(_) | LoroValue::Binary(_) => serde_yaml::Value::Null,
        LoroValue::Bool(b) => serde_yaml::Value::Bool(*b),
        LoroValue::I64(i) => serde_yaml::Value::Number((*i).into()),
        LoroValue::Double(d) => serde_yaml::Value::Number((*d).into()),
        LoroValue::String(s) => serde_yaml::Value::String(s.to_string()),
        LoroValue::List(l) => serde_yaml::Value::Sequence(l.iter().map(loro_to_yaml).collect()),
        LoroValue::Map(m) => {
            let sorted: BTreeMap<&String, &LoroValue> = m.iter().collect();
            serde_yaml::Value::Mapping(
                sorted
                    .into_iter()
                    .map(|(k, v)| (serde_yaml::Value::String(k.clone()), loro_to_yaml(v)))
                    .collect(),
            )
        }
    }
}

/// One top-level frontmatter entry of a previous file: its key (when it
/// is a key line, not leading comments) and its exact lines.
struct Chunk<'a> {
    key: Option<String>,
    lines: &'a str,
}

/// Split a frontmatter block into top-level entries. A key line starts a
/// new entry; everything else (continuations, nested lists at column 0,
/// comments, blank lines) belongs to the entry before it.
fn chunks(yaml: &str) -> Vec<Chunk<'_>> {
    let mut out: Vec<Chunk<'_>> = Vec::new();
    let mut start = 0;
    let mut key: Option<String> = None;
    let mut pos = 0;
    for line in yaml.split_inclusive('\n') {
        if let Some(k) = key_of_line(line) {
            if pos > start || key.is_some() {
                out.push(Chunk {
                    key: key.take(),
                    lines: &yaml[start..pos],
                });
            }
            start = pos;
            key = Some(k);
        }
        pos += line.len();
    }
    if pos > start || key.is_some() {
        out.push(Chunk {
            key,
            lines: &yaml[start..pos],
        });
    }
    out
}

fn key_of_line(line: &str) -> Option<String> {
    let first = line.chars().next()?;
    if first.is_whitespace() || matches!(first, '-' | '#' | '?' | '[' | '{') {
        return None;
    }
    let colon = line.find(':')?;
    let raw = line[..colon].trim();
    let unquoted = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(raw);
    (!unquoted.is_empty()).then(|| unquoted.to_owned())
}

/// The value a single chunk states (parsed on its own).
fn chunk_value(c: &Chunk<'_>) -> Option<LoroValue> {
    let key = c.key.as_ref()?;
    let props = parse_yaml_props(c.lines)?;
    props.get(key).cloned()
}

fn emit_entry(key: &str, v: &LoroValue) -> String {
    let mut m = serde_yaml::Mapping::new();
    m.insert(serde_yaml::Value::String(key.to_owned()), loro_to_yaml(v));
    let s = serde_yaml::to_string(&serde_yaml::Value::Mapping(m)).unwrap_or_default();
    if s.ends_with('\n') {
        s
    } else {
        format!("{s}\n")
    }
}

/// Write props + body as a file.
///
/// With `previous` (the file as it is now), the frontmatter keeps its
/// order and every unchanged entry's exact lines; changed entries are
/// rewritten in place, removed ones dropped, new ones appended in key
/// order. With no props the file has no frontmatter — unless the
/// previous file had an empty block, which is kept as it was.
#[must_use]
pub fn materialise(props: &Props, body: &str, previous: Option<&str>) -> String {
    let prev_yaml = previous
        .and_then(|p| fences(p).map(|f| &p[f.yaml.0..f.yaml.1]))
        .filter(|_| previous.is_some_and(|p| !matches!(parse(p).front, Front::Invalid)));
    let mut yaml = String::new();
    let mut written: Vec<&str> = Vec::new();
    let mut prev_had_keys = false;
    if let Some(prev) = prev_yaml {
        // The fence strips the block's final newline; give it back so
        // every chunk ends in one.
        let prev_owned = format!("{prev}\n");
        for c in chunks(&prev_owned) {
            match &c.key {
                None => {
                    if !c.lines.trim().is_empty() {
                        yaml.push_str(c.lines);
                    }
                }
                Some(k) => {
                    prev_had_keys = true;
                    let Some((key, now)) = props.get_key_value(k) else {
                        continue; // removed
                    };
                    written.push(key.as_str());
                    if chunk_value(&c).as_ref() == Some(now) {
                        yaml.push_str(c.lines);
                    } else {
                        yaml.push_str(&emit_entry(k, now));
                    }
                }
            }
        }
    }
    for (k, v) in props {
        if !written.contains(&k.as_str()) {
            yaml.push_str(&emit_entry(k, v));
        }
    }
    let trimmed = yaml.trim_end_matches('\n');
    if props.is_empty() {
        let keep_empty_block = prev_yaml.is_some() && !prev_had_keys;
        return if keep_empty_block {
            format!("---\n{}---\n{body}", with_newline(trimmed))
        } else {
            body.to_owned()
        };
    }
    format!("---\n{}---\n{body}", with_newline(trimmed))
}

fn with_newline(s: &str) -> String {
    if s.is_empty() {
        String::new()
    } else {
        format!("{s}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props_of(text: &str) -> Props {
        parse(text).props().expect("valid frontmatter")
    }

    const NOTE: &str = "---\n# kept comment\ntitle: \"Mix notes\"\ntags:\n- mix\n- vocals\nstatus: open\n---\n# Mix notes\n\nBody.\n";

    // t[verify offline.doc.shape] — a file reads as props + body and
    // writes back byte for byte
    #[test]
    fn a_file_round_trips_byte_for_byte() {
        let p = parse(NOTE);
        assert_eq!(p.body, "# Mix notes\n\nBody.\n");
        let props = p.props().unwrap();
        assert_eq!(props.get("title"), Some(&LoroValue::from("Mix notes")));
        assert_eq!(
            props.get("tags"),
            Some(&LoroValue::List(
                vec![LoroValue::from("mix"), LoroValue::from("vocals")].into()
            ))
        );
        assert_eq!(materialise(&props, p.body, Some(NOTE)), NOTE);
    }

    // t[verify offline.doc.materialise] — a changed prop rewrites only
    // its own lines; order, comments and quoting elsewhere stay
    #[test]
    fn changing_one_prop_rewrites_only_its_lines() {
        let mut props = props_of(NOTE);
        props.insert("status".into(), LoroValue::from("done"));
        props.insert("due".into(), LoroValue::from("2026-10-20"));
        props.remove("tags");
        let out = materialise(&props, "Body.\n", Some(NOTE));
        assert_eq!(
            out,
            "---\n# kept comment\ntitle: \"Mix notes\"\nstatus: done\ndue: 2026-10-20\n---\nBody.\n"
        );
        // And it reads back as what was written.
        assert_eq!(props_of(&out), props);
    }

    #[test]
    fn no_frontmatter_is_all_body_and_stays_that_way() {
        let p = parse("just text\n");
        assert_eq!(p.front, Front::None);
        assert_eq!(
            materialise(&Props::new(), p.body, Some("just text\n")),
            "just text\n"
        );
        // A first prop brings a block.
        let mut props = Props::new();
        props.insert("id".into(), LoroValue::from("abc"));
        assert_eq!(materialise(&props, "x", None), "---\nid: abc\n---\nx");
    }

    #[test]
    fn an_empty_block_is_kept_and_a_broken_one_is_invalid() {
        let empty = "---\n---\nbody";
        assert_eq!(parse(empty).props(), Some(Props::new()));
        assert_eq!(parse(empty).body, "body");
        assert_eq!(materialise(&Props::new(), "body", Some(empty)), empty);
        let broken = "---\ntitle: [unclosed\n---\nbody";
        assert_eq!(parse(broken).front, Front::Invalid);
        // Read as all body, so none of it is lost.
        assert_eq!(parse(broken).body, broken);
        assert_eq!(materialise(&Props::new(), broken, Some(broken)), broken);
        assert!(is_mid_edit(broken));
        assert!(is_mid_edit("---\ntitle: x\n"));
        assert!(!is_mid_edit(NOTE));
        assert!(!is_mid_edit("plain"));
    }

    #[test]
    fn a_task_file_keeps_its_blank_separator_in_the_body() {
        let task = "---\ntype: task\ntitle: Call venue\n---\n\nAsk about load-in.\n";
        let p = parse(task);
        assert_eq!(p.body, "\nAsk about load-in.\n");
        assert_eq!(materialise(&p.props().unwrap(), p.body, Some(task)), task);
    }

    #[test]
    fn nested_values_and_numbers_survive() {
        let text = "---\ntimeEstimate: 30\nratio: 0.5\ndone: false\nworkflow:\n  parent: x\n  depth: 2\n---\n";
        let props = props_of(text);
        assert_eq!(props.get("timeEstimate"), Some(&LoroValue::I64(30)));
        assert_eq!(props.get("ratio"), Some(&LoroValue::Double(0.5)));
        assert_eq!(props.get("done"), Some(&LoroValue::Bool(false)));
        assert!(matches!(props.get("workflow"), Some(LoroValue::Map(_))));
        // Fresh (no previous) output reads back to the same props.
        let fresh = materialise(&props, "", None);
        assert_eq!(props_of(&fresh), props);
        assert_eq!(materialise(&props, "", Some(text)), text);
    }
}
