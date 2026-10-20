//! A task as a vault document (`docs/spec/offline.md`,
//! `offline.doc.shape`): the file text ⇄ [`TaskInfo`] mapping every
//! host shares — the server's `TaskBackend`, the device's record book
//! and the shell — so a task reads and writes the same everywhere.
//!
//! A task's Loro document is the file's `props` (its frontmatter keys)
//! plus `body` (everything after the close fence — a blank line, then
//! [`TaskInfo::details`]). The text side lives here and is pure; the
//! document side (`vault_doc::merge_text`) applies a change as a change:
//!
//! - [`serialize_task`] / [`parse_task`] — `TaskInfo` ⇄ markdown, the
//!   TaskNotes shape (`type: task` first, default fields omitted).
//! - [`edit_texts`] — what an edit of a task *changes*: the pair of
//!   texts (`base` → `next`) a document merges, so only the fields that
//!   differ between them are set (each by key) and only the description's
//!   own edit is applied to the body. Every frontmatter key the
//!   document holds that is not a task field (an Obsidian plugin's, a
//!   person's own) is in neither text, so it is never touched.
//!
//! Wasm-clean: serde_yaml + the model.

use serde_json::Value;
use thiserror::Error;

use crate::model::{TaskInfo, TimeEntry};

/// Why a file is not a readable task.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("page has no frontmatter")]
    NoFrontmatter,
    #[error("frontmatter is not a YAML mapping")]
    NotAMapping,
    #[error("frontmatter parse: {0}")]
    Yaml(String),
}

// ── text → TaskInfo ─────────────────────────────────────────────────

/// Whether `raw` reads as a task: `type: task`, or `task` in `tags`.
#[must_use]
pub fn looks_like_task(raw: &str) -> bool {
    let Some((fm, _body)) = split_frontmatter(raw) else {
        return false;
    };
    let Ok(map) = serde_yaml::from_str::<serde_yaml::Mapping>(fm) else {
        return false;
    };
    if map.get("type").and_then(serde_yaml::Value::as_str) == Some("task") {
        return true;
    }
    map.get("tags")
        .and_then(serde_yaml::Value::as_sequence)
        .is_some_and(|seq| seq.iter().any(|v| v.as_str() == Some("task")))
}

/// The file name a path's task falls back to for its title.
#[must_use]
pub fn basename(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".md").unwrap_or(name)
}

/// Parse a task file (frontmatter + body). `rel_path` becomes
/// [`TaskInfo::path`] and seeds the fallback id; `basename` fills a
/// missing title.
pub fn parse_task(rel_path: &str, basename: &str, raw: &str) -> Result<TaskInfo, ParseError> {
    let (fm, body) = split_frontmatter(raw).ok_or(ParseError::NoFrontmatter)?;
    let map: serde_yaml::Mapping =
        serde_yaml::from_str(fm).map_err(|e| ParseError::Yaml(e.to_string()))?;

    let title = take_str(&map, "title").unwrap_or_else(|| basename.to_string());
    let status = take_str(&map, "status").unwrap_or_else(|| "open".into());
    let priority = take_str(&map, "priority").unwrap_or_else(|| "normal".into());

    let time_estimate = map
        .get("timeEstimate")
        .and_then(serde_yaml::Value::as_u64)
        .and_then(|n| u32::try_from(n).ok());

    let time_entries = map
        .get("timeEntries")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|row| {
                    let row_map = row.as_mapping()?;
                    let start = row_map
                        .get("startTime")?
                        .as_str()
                        .and_then(|s| s.parse().ok())?;
                    let end = row_map
                        .get("endTime")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok());
                    Some(TimeEntry {
                        start_time: start,
                        end_time: end,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // Backfill id when the frontmatter lacks one: a deterministic
    // namespace uuid from the path, so the same file resolves to the same
    // id across machines until the next save writes it.
    let id = take_str(&map, "id")
        .and_then(|s| uuid::Uuid::parse_str(&s).ok())
        .unwrap_or_else(|| uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, rel_path.as_bytes()));

    Ok(TaskInfo {
        id,
        path: rel_path.to_string(),
        title,
        status,
        priority,
        due: take_str(&map, "due"),
        scheduled: take_str(&map, "scheduled"),
        tags: crate::model::StringList(take_string_list(&map, "tags")),
        contexts: crate::model::StringList(take_string_list(&map, "contexts")),
        projects: crate::model::StringList(take_string_list(&map, "projects")),
        project_id: take_str(&map, "projectId").and_then(|s| uuid::Uuid::parse_str(&s).ok()),
        milestone_id: take_str(&map, "milestoneId").and_then(|s| uuid::Uuid::parse_str(&s).ok()),
        time_estimate,
        time_entries: crate::model::TimeEntries(time_entries),
        recurrence: take_str(&map, "recurrence"),
        recurrence_anchor: take_str(&map, "recurrence_anchor"),
        complete_instances: crate::model::StringList(take_string_list(&map, "complete_instances")),
        completed_date: take_str(&map, "completedDate").and_then(|s| s.parse().ok()),
        agent_profile: take_str(&map, "agentProfile").unwrap_or_default(),
        dispatched_agent_tasks: crate::model::StringList(take_string_list(
            &map,
            "dispatchedAgentTasks",
        )),
        date_created: take_str(&map, "dateCreated").and_then(|s| s.parse().ok()),
        date_modified: take_str(&map, "dateModified").and_then(|s| s.parse().ok()),
        // The writer sets the body off with one blank line; take exactly
        // that one back (see [`serialize_task`]), so details round-trip.
        details: body.strip_prefix('\n').unwrap_or(body).to_string(),
        workflow: take_workflow_attrs(&map),
    })
}

/// [`parse_task`] for a document's rendered text at `path`, when it is
/// a task.
#[must_use]
pub fn task_of(path: &str, raw: &str) -> Option<TaskInfo> {
    if !looks_like_task(raw) {
        return None;
    }
    parse_task(path, basename(path), raw).ok()
}

/// The optional nested `workflow:` mapping. `None` when absent or
/// unreadable (so an empty block doesn't stamp `Some(default)`).
fn take_workflow_attrs(map: &serde_yaml::Mapping) -> Option<crate::model::WorkflowAttrs> {
    let value = map.get(serde_yaml::Value::String("workflow".into()))?;
    serde_yaml::from_value(value.clone()).ok()
}

/// Split a markdown source into `(frontmatter_yaml, body)`. The block
/// must open the file with `---` and close on a `---` line of its own.
#[must_use]
pub fn split_frontmatter(src: &str) -> Option<(&str, &str)> {
    let rest = src.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some((&rest[..end], &rest[end + 5..]))
}

fn take_str(map: &serde_yaml::Mapping, key: &str) -> Option<String> {
    map.get(key).and_then(|v| match v {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

fn take_string_list(map: &serde_yaml::Mapping, key: &str) -> Vec<String> {
    match map.get(key) {
        Some(serde_yaml::Value::Sequence(seq)) => seq
            .iter()
            .filter_map(|item| match item {
                serde_yaml::Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        // Tolerate `tags: foo` shorthand (single string).
        Some(serde_yaml::Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// JSON variant of `take_str`, for callers holding a page's
/// frontmatter as JSON.
#[must_use]
pub fn json_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

// ── TaskInfo → text ─────────────────────────────────────────────────

/// The frontmatter block of `task` (without fences): `type: task`
/// first, then every non-default field.
fn frontmatter_yaml(task: &TaskInfo) -> Result<String, String> {
    let mut wrapper = serde_yaml::Mapping::new();
    wrapper.insert("type".into(), "task".into());
    let fields = serde_yaml::to_value(task).map_err(|e| e.to_string())?;
    if let serde_yaml::Value::Mapping(map) = fields {
        for (k, v) in map {
            wrapper.insert(k, v);
        }
    }
    serde_yaml::to_string(&serde_yaml::Value::Mapping(wrapper)).map_err(|e| e.to_string())
}

fn with_body(task: &TaskInfo, prefix: &str) -> Result<String, String> {
    let yaml = frontmatter_yaml(task)?;
    let body = if task.details.is_empty() {
        String::new()
    } else {
        format!("{prefix}{}", task.details)
    };
    Ok(format!("---\n{yaml}---\n{body}"))
}

/// A task as its markdown file: frontmatter block + body. A non-empty
/// description is set off by ONE blank line, which [`parse_task`] takes
/// back off — so `details` round-trips byte for byte.
pub fn serialize_task(task: &TaskInfo) -> Result<String, String> {
    with_body(task, "\n")
}

/// How the body of `raw` sets the description off: `"\n"` for the
/// writer's convention (and an empty body), `""` for a hand-written
/// file whose text starts right after the fence.
fn body_prefix(raw: &str) -> &'static str {
    match split_frontmatter(raw) {
        Some((_, body)) if !body.is_empty() && !body.starts_with('\n') => "",
        _ => "\n",
    }
}

// t[impl offline.doc.materialise] — a service edit of a task is text
// coming in: only what it changed is merged into the document
/// The two texts an edit of a task merges into its document: `base`
/// (the task as the editor saw it) and `next` (as they left it), both
/// laid out like `current` (the document's text now, when known) so the
/// description's edit lands at its place in the body.
///
/// Merging them (`vault_doc::merge_text(doc, &base, &next)`) sets each
/// field that differs between `base` and `next`, by key, and applies the
/// description's edit to the body; a field the editor did not change —
/// and every key that is not a task field — is left as the document has
/// it, so a concurrent change made elsewhere survives.
pub fn edit_texts(
    current: Option<&str>,
    base: &TaskInfo,
    next: &TaskInfo,
) -> Result<(String, String), String> {
    let prefix = current.map_or("\n", body_prefix);
    Ok((with_body(base, prefix)?, with_body(next, prefix)?))
}

// ── paths ───────────────────────────────────────────────────────────

/// A vault-relative path for a new task from its title: `Task/<slug>.md`,
/// or under `folder`.
#[must_use]
pub fn default_task_path(title: &str, folder: Option<&str>) -> String {
    let slug = slugify(title);
    match folder {
        Some(f) => format!("{}/{slug}.md", f.trim_end_matches('/')),
        None => format!("Task/{slug}.md"),
    }
}

// t[impl offline.doc.create] — a task made on a device names its own file
/// A path for a task created on a device, before any server has seen
/// it: the title's slug plus a short tag no other device will pick
/// (`Task/call-the-venue-3f9a.md`), from `tag` (any random hex).
#[must_use]
pub fn device_task_path(title: &str, tag: &str) -> String {
    let short: String = tag
        .chars()
        .filter(char::is_ascii_hexdigit)
        .take(4)
        .collect();
    format!("Task/{}-{short}.md", slugify(title))
}

/// Lowercase, spaces → `-`, non-alphanumerics dropped; `task` when
/// nothing is left.
#[must_use]
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("task");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(title: &str) -> TaskInfo {
        let mut t = crate::capture::capture(title);
        t.path = "Task/mix.md".into();
        t
    }

    // t[verify offline.doc.shape] — a task round-trips through its file
    #[test]
    fn a_task_round_trips_through_its_text() {
        let mut t = task("Mix the single");
        t.status = "in-progress".into();
        t.details = "## Plan\n\n- [ ] Comp vocals\n".into();
        t.project_id = Some(uuid::Uuid::new_v4());
        let text = serialize_task(&t).unwrap();
        assert!(text.starts_with("---\ntype: task\n"), "{text}");
        let back = task_of(&t.path, &text).expect("a task");
        assert_eq!(back, t);
        assert_eq!(serialize_task(&back).unwrap(), text);
    }

    #[test]
    fn edit_texts_differ_only_where_the_edit_did() {
        let base = task("Mix the single");
        let mut next = base.clone();
        next.status = "done".into();
        let (b, n) = edit_texts(None, &base, &next).unwrap();
        let changed: Vec<_> = b.lines().zip(n.lines()).filter(|(x, y)| x != y).collect();
        assert_eq!(changed, vec![("status: open", "status: done")]);
        // A hand-written body (no blank line after the fence) keeps its
        // layout, so the description's edit lands where it is.
        let hand = "---\ntype: task\ntitle: Mix\n---\nBody\n";
        let mut b2 = task_of("Task/mix.md", hand).unwrap();
        b2.path = "Task/mix.md".into();
        let mut n2 = b2.clone();
        n2.details = "Body!\n".into();
        let (bt, nt) = edit_texts(Some(hand), &b2, &n2).unwrap();
        assert!(bt.ends_with("---\nBody\n") && nt.ends_with("---\nBody!\n"));
    }

    // t[verify offline.doc.shape] — a task's document is props + body;
    // t[verify offline.doc.materialise] — an edit merged as a change
    // touches only the fields it changed: keys that are not task fields
    // (and a field changed elsewhere meanwhile) survive
    #[test]
    fn an_edit_merged_into_the_document_keeps_unknown_keys_and_other_changes() {
        let file = "---\ntype: task\ntitle: Mix\nstatus: open\nkanban-lane: west\n---\n\nComp the vocals.\n";
        let doc = vault_doc::doc::new_doc();
        vault_doc::adopt_text(&doc, file);
        vault_doc::commit_as(&doc, "seed");
        let base = task_of("Task/mix.md", file).unwrap();
        // Meanwhile another replica retitles the task.
        vault_doc::doc::set_prop(&doc, "title", "Final mix".into());
        doc.commit();
        // This editor ticks it off and adds a line to the description.
        let mut next = base.clone();
        next.status = "done".into();
        next.details = "Comp the vocals.\nBounce stems.\n".into();
        let current = vault_doc::render(&doc, Some(file));
        let (b, n) = edit_texts(Some(&current), &base, &next).unwrap();
        vault_doc::merge_text(&doc, &b, &n);
        vault_doc::commit_as(&doc, "task-service");
        let out = vault_doc::render(&doc, Some(file));
        assert_eq!(
            out,
            "---\ntype: task\ntitle: Final mix\nstatus: done\nkanban-lane: west\n---\n\nComp the vocals.\nBounce stems.\n"
        );
        let t = task_of("Task/mix.md", &out).unwrap();
        assert_eq!((t.title.as_str(), t.status.as_str()), ("Final mix", "done"));
        assert_eq!(t.id, base.id);
    }

    #[test]
    fn device_paths_are_tagged() {
        assert_eq!(
            device_task_path("Call the venue!", "3f9a0c"),
            "Task/call-the-venue-3f9a.md"
        );
        assert_eq!(default_task_path("Buy milk", None), "Task/buy-milk.md");
    }
}
