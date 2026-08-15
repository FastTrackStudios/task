//! `--json` output + flexible-reference resolution helpers shared by
//! every CLI surface (timer / finance / cycle / task / issue /
//! project / goal / milestone / workstream / threads / auth).
//!
//! Lives in its own module (rather than `main.rs`) so concurrent
//! edits to the giant command-arm file don't collide on helper code.
//!
//! Two concerns:
//! 1. JSON rendering — entity → `serde_json::Value`, including
//!    hand-built objects for storage models that don't derive
//!    `Serialize` (the architect-generated SeaORM `Model`s).
//! 2. Reference resolution — turning a human-typed `--task` /
//!    `--project` / `--goal` / … value (uuid, id prefix, vault path,
//!    or name) into the entity, with pure matching functions kept
//!    separate from the async client plumbing so they unit-test
//!    cheaply. Failures are tagged with the [`crate::errors`]
//!    taxonomy (not-found vs ambiguous) so `main()` can map them to
//!    stable exit codes.

use crate::errors;
use eyre::eyre;

// ---------------------------------------------------------------
// JSON rendering
// ---------------------------------------------------------------

/// Print any serializable value as pretty JSON on stdout.
pub fn print_json<T: serde::Serialize>(value: &T) -> eyre::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| eyre!("json: {e}"))?
    );
    Ok(())
}

/// `WorkSession` entity → JSON, plus derived `seconds_elapsed`
/// (wall-clock for an open timer, span for a closed one). Mutating
/// verbs emit exactly this shape.
pub fn session_json(s: &timer_proto::WorkSession) -> serde_json::Value {
    let mut v = serde_json::to_value(s).unwrap_or(serde_json::Value::Null);
    if let serde_json::Value::Object(map) = &mut v {
        map.insert("seconds_elapsed".into(), session_seconds(s).into());
    }
    v
}

/// [`session_json`] + joined `task_title` / `project_title` when the
/// caller managed to resolve them (`timer active --json`). `None`
/// joins are omitted rather than emitted as null — they mean
/// "unresolvable right now", not "no task".
pub fn session_json_joined(
    s: &timer_proto::WorkSession,
    task_title: Option<String>,
    project_title: Option<String>,
) -> serde_json::Value {
    let mut v = session_json(s);
    if let serde_json::Value::Object(map) = &mut v {
        if let Some(t) = task_title {
            map.insert("task_title".into(), t.into());
        }
        if let Some(p) = project_title {
            map.insert("project_title".into(), p.into());
        }
    }
    v
}

/// Elapsed seconds of a session — `now - start` while open,
/// `end - start` once closed. Clamped at zero.
pub fn session_seconds(s: &timer_proto::WorkSession) -> i64 {
    s.end_time
        .unwrap_or_else(chrono::Utc::now)
        .signed_duration_since(s.start_time)
        .num_seconds()
        .max(0)
}

/// Timer tag → JSON. Takes the proto type; storage `TagModel`s
/// convert via the generated `From<Model>` first.
pub fn tag_json(t: &timer_proto::Tag) -> serde_json::Value {
    serde_json::to_value(t).unwrap_or(serde_json::Value::Null)
}

#[cfg(feature = "plugin-finance")]
/// Invoice → JSON (wire shape). The proto struct doesn't derive
/// `Serialize`, so build the object by hand (status rendered as the
/// same lowercase slug the table view uses). Keys unchanged from
/// the storage-`Model` era.
pub fn invoice_json(r: &finance_proto::Invoice) -> serde_json::Value {
    serde_json::json!({
        "id": r.id,
        "number": r.number,
        "status": format!("{:?}", r.status).to_lowercase(),
        "issue_date": r.issue_date,
        "due_date": r.due_date,
        "currency": r.currency,
        "subtotal_minor": r.subtotal_minor,
        "total_minor": r.total_minor,
        "amount_paid_minor": r.amount_paid_minor,
        "balance_minor": r.balance_minor,
        "party_id": r.party_id,
        "book_id": r.book_id,
        "line_items": serde_json::to_value(&r.line_items).unwrap_or(serde_json::Value::Null),
    })
}

/// One cycle → JSON with the derived "how far through" fields the
/// human rendering prints (`cycle current`).
pub fn cycle_json(c: &cycle::Cycle, today: chrono::NaiveDate) -> serde_json::Value {
    let total = (c.end_date - c.start_date).num_days() + 1;
    let elapsed = (today - c.start_date).num_days() + 1;
    serde_json::json!({
        "cycle": c,
        "label": format!("{}-Q{}-C{}", c.year, c.quarter, c.ordinal),
        "today": today,
        "day": elapsed,
        "total_days": total,
        "pct": (elapsed as f64) * 100.0 / (total as f64),
    })
}

// ---------------------------------------------------------------
// Pure matching (unit-tested; no clients)
// ---------------------------------------------------------------

/// Outcome of a prefix scan over candidate rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixMatch {
    /// Nothing matched.
    None,
    /// Exactly one row matched — its index.
    Unique(usize),
    /// More than one row matched — how many.
    Ambiguous(usize),
}

/// Case-insensitive uuid-prefix scan. An empty needle matches
/// nothing (it would otherwise "match" every row).
pub fn match_uuid_prefix(ids: &[uuid::Uuid], needle: &str) -> PrefixMatch {
    let needle = needle.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return PrefixMatch::None;
    }
    let hits: Vec<usize> = ids
        .iter()
        .enumerate()
        .filter(|(_, id)| id.to_string().starts_with(&needle))
        .map(|(i, _)| i)
        .collect();
    match hits.len() {
        0 => PrefixMatch::None,
        1 => PrefixMatch::Unique(hits[0]),
        n => PrefixMatch::Ambiguous(n),
    }
}

/// Candidate row for entity matching: `(id, name, key)` where `key`
/// is the secondary exact-match handle — a vault path for tasks /
/// projects / goals / milestones, a slug for auth orgs.
pub type Candidate = (uuid::Uuid, String, String);

/// Why [`match_entity`] failed — split so callers can map onto the
/// exit-code taxonomy (`NotFound` → 4, `Ambiguous` → 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchFailure {
    NotFound(String),
    Ambiguous(String),
}

impl MatchFailure {
    /// Tag onto the error taxonomy as a `resolve <noun>` failure.
    pub fn into_report(self, noun: &str, target: &str) -> eyre::Report {
        match self {
            MatchFailure::NotFound(msg) => errors::not_found(format!("resolve {noun}"), target)
                .cause(msg)
                .report(),
            MatchFailure::Ambiguous(msg) => errors::conflict(format!("resolve {noun}"), target)
                .cause(msg)
                .hint("disambiguate with the full name, path, or uuid")
                .report(),
        }
    }
}

/// Match a non-UUID target against an org's entity rows.
/// Resolution order (first level with a hit wins):
/// 1. exact key (path / slug),
/// 2. exact name (case-insensitive),
/// 3. unique name prefix (case-insensitive),
/// 4. unique id prefix.
///
/// Returns the index of the matched row, or a structured failure
/// ("no match" / "ambiguous, here are the candidates").
pub fn match_entity(rows: &[Candidate], target: &str, noun: &str) -> Result<usize, MatchFailure> {
    let t = target.trim();
    if t.is_empty() {
        return Err(MatchFailure::NotFound(format!("empty {noun} reference")));
    }
    // 1. exact key (path / slug)
    if let Some(i) = rows.iter().position(|(_, _, key)| key == t) {
        return Ok(i);
    }
    // 2. exact name, case-insensitive
    let lower = t.to_lowercase();
    let name_eq: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, (_, name, _))| name.to_lowercase() == lower)
        .map(|(i, _)| i)
        .collect();
    match name_eq.len() {
        1 => return Ok(name_eq[0]),
        n if n > 1 => {
            return Err(MatchFailure::Ambiguous(format!(
                "`{target}` matches {n} {noun} names — use the path or uuid"
            )));
        }
        _ => {}
    }
    // 3. unique name prefix, case-insensitive
    let name_pre: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, (_, name, _))| name.to_lowercase().starts_with(&lower))
        .map(|(i, _)| i)
        .collect();
    match name_pre.len() {
        1 => return Ok(name_pre[0]),
        n if n > 1 => {
            let names: Vec<&str> = name_pre
                .iter()
                .take(5)
                .map(|&i| rows[i].1.as_str())
                .collect();
            return Err(MatchFailure::Ambiguous(format!(
                "`{target}` is an ambiguous {noun} name prefix ({n} matches: {})",
                names.join(", ")
            )));
        }
        _ => {}
    }
    // 4. unique id prefix
    let ids: Vec<uuid::Uuid> = rows.iter().map(|(id, _, _)| *id).collect();
    match match_uuid_prefix(&ids, t) {
        PrefixMatch::Unique(i) => Ok(i),
        PrefixMatch::Ambiguous(n) => Err(MatchFailure::Ambiguous(format!(
            "`{target}` matches {n} {noun} ids — use the full uuid"
        ))),
        PrefixMatch::None => Err(MatchFailure::NotFound(format!(
            "`{target}` matches no {noun} path, name, or id prefix"
        ))),
    }
}

// ---------------------------------------------------------------
// Async resolvers (thin client plumbing over the pure matchers)
// ---------------------------------------------------------------

/// Resolve a `--task` / issue value — full UUID, id prefix,
/// vault-relative path, or title (exact / unique prefix) — to its
/// [`task::TaskInfo`]. Validates the task exists (errors otherwise).
pub async fn resolve_task_flexible(
    client: &task::TaskServiceClient,
    target: &str,
) -> eyre::Result<task::TaskInfo> {
    if let Ok(id) = uuid::Uuid::parse_str(target) {
        return client.get(id).await.map_err(|e| {
            errors::not_found("resolve task", target)
                .cause(format!("{e:?}"))
                .report()
        });
    }
    // Path-shaped input — try the exact lookup first.
    let md_ext = std::path::Path::new(target)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
    if target.contains('/') || md_ext {
        if let Ok(t) = client.get_by_path(target.to_owned()).await {
            return Ok(t);
        }
    }
    let rows = client
        .list()
        .await
        .map_err(|e| eyre!("list tasks: {e:?}"))?;
    let cands: Vec<Candidate> = rows
        .iter()
        .map(|t| (t.id, t.title.clone(), t.path.clone()))
        .collect();
    match match_entity(&cands, target, "task") {
        Ok(i) => Ok(rows.into_iter().nth(i).expect("index from enumerate")),
        Err(fail) => Err(fail.into_report("task", target)),
    }
}

/// Resolve a `--project` value — uuid, vault path, title, or a
/// unique prefix of either — to its [`project::ProjectInfo`].
pub async fn resolve_project_flexible(
    client: &project::ProjectServiceClient,
    target: &str,
) -> eyre::Result<project::ProjectInfo> {
    if let Ok(id) = uuid::Uuid::parse_str(target) {
        return client.get(id).await.map_err(|e| {
            errors::not_found("resolve project", target)
                .cause(format!("{e:?}"))
                .report()
        });
    }
    if let Ok(p) = client.get_by_path(target.to_owned()).await {
        return Ok(p);
    }
    let rows = client
        .list()
        .await
        .map_err(|e| eyre!("list projects: {e:?}"))?;
    let cands: Vec<Candidate> = rows
        .iter()
        .map(|p| (p.id, p.title.clone(), p.path.clone()))
        .collect();
    match match_entity(&cands, target, "project") {
        Ok(i) => Ok(rows.into_iter().nth(i).expect("index from enumerate")),
        Err(fail) => Err(fail.into_report("project", target)),
    }
}

/// Resolve a `--goal` value — uuid, vault path, title, or a unique
/// prefix of either — to its [`goal::Goal`].
pub async fn resolve_goal_flexible(
    client: &goal::GoalServiceClient,
    target: &str,
) -> eyre::Result<goal::Goal> {
    if let Ok(id) = uuid::Uuid::parse_str(target) {
        return client.get(id).await.map_err(|e| {
            errors::not_found("resolve goal", target)
                .cause(format!("{e:?}"))
                .report()
        });
    }
    if let Ok(g) = client.get_by_path(target.to_owned()).await {
        return Ok(g);
    }
    let rows = client
        .list()
        .await
        .map_err(|e| eyre!("list goals: {e:?}"))?;
    let cands: Vec<Candidate> = rows
        .iter()
        .map(|g| (g.id, g.title.clone(), g.path.clone()))
        .collect();
    match match_entity(&cands, target, "goal") {
        Ok(i) => Ok(rows.into_iter().nth(i).expect("index from enumerate")),
        Err(fail) => Err(fail.into_report("goal", target)),
    }
}

/// Resolve a `--milestone` value — uuid, vault path, title, or a
/// unique prefix of either — to its [`milestone::Milestone`].
pub async fn resolve_milestone_flexible(
    client: &milestone::MilestoneServiceClient,
    target: &str,
) -> eyre::Result<milestone::Milestone> {
    if let Ok(id) = uuid::Uuid::parse_str(target) {
        return client.get(id).await.map_err(|e| {
            errors::not_found("resolve milestone", target)
                .cause(format!("{e:?}"))
                .report()
        });
    }
    if let Ok(m) = client.get_by_path(target.to_owned()).await {
        return Ok(m);
    }
    let rows = client
        .list()
        .await
        .map_err(|e| eyre!("list milestones: {e:?}"))?;
    let cands: Vec<Candidate> = rows
        .iter()
        .map(|m| (m.id, m.title.clone(), m.path.clone()))
        .collect();
    match match_entity(&cands, target, "milestone") {
        Ok(i) => Ok(rows.into_iter().nth(i).expect("index from enumerate")),
        Err(fail) => Err(fail.into_report("milestone", target)),
    }
}

/// Resolve a `--workstream` value — uuid, vault path, title, or a
/// unique prefix of either — to its [`workstream::Workstream`].
pub async fn resolve_workstream_flexible(
    client: &workstream::WorkstreamServiceClient,
    target: &str,
) -> eyre::Result<workstream::Workstream> {
    if let Ok(id) = uuid::Uuid::parse_str(target) {
        return client.get(id).await.map_err(|e| {
            errors::not_found("resolve workstream", target)
                .cause(format!("{e:?}"))
                .report()
        });
    }
    if let Ok(w) = client.get_by_path(target.to_owned()).await {
        return Ok(w);
    }
    let rows = client
        .list(None)
        .await
        .map_err(|e| eyre!("list workstreams: {e:?}"))?;
    let cands: Vec<Candidate> = rows
        .iter()
        .map(|w| (w.id, w.title.clone(), w.path.clone()))
        .collect();
    match match_entity(&cands, target, "workstream") {
        Ok(i) => Ok(rows.into_iter().nth(i).expect("index from enumerate")),
        Err(fail) => Err(fail.into_report("workstream", target)),
    }
}

/// Resolve an optional `--project` flag to `(project_id,
/// known_path)`. A raw UUID short-circuits without dialing vox —
/// plain local timer use keeps working offline — at the cost of an
/// unknown path (caller falls back to the vault scan). Anything
/// else establishes the project client via `connect` and resolves
/// name / path / prefix server-side.
pub async fn resolve_project_arg<F, Fut>(
    target: Option<&str>,
    connect: F,
) -> eyre::Result<(Option<uuid::Uuid>, Option<String>)>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = eyre::Result<project::ProjectServiceClient>>,
{
    let Some(target) = target else {
        return Ok((None, None));
    };
    if let Ok(id) = uuid::Uuid::parse_str(target) {
        return Ok((Some(id), None));
    }
    let client = connect().await?;
    let info = resolve_project_flexible(&client, target).await?;
    Ok((Some(info.id), Some(info.path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(n: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(n)
    }

    // -- match_uuid_prefix --------------------------------------

    #[test]
    fn uuid_prefix_unique() {
        let ids = vec![uid(0xaa00), uid(0xbb00)];
        // 0xaa00 → "00000000-0000-0000-0000-00000000aa00"
        assert_eq!(
            match_uuid_prefix(&ids, "00000000-0000-0000-0000-00000000aa"),
            PrefixMatch::Unique(0)
        );
    }

    #[test]
    fn uuid_prefix_ambiguous_and_none() {
        let ids = vec![uid(1), uid(2)];
        assert_eq!(match_uuid_prefix(&ids, "0000"), PrefixMatch::Ambiguous(2));
        assert_eq!(match_uuid_prefix(&ids, "ffff"), PrefixMatch::None);
    }

    #[test]
    fn uuid_prefix_empty_matches_nothing() {
        let ids = vec![uid(1)];
        assert_eq!(match_uuid_prefix(&ids, ""), PrefixMatch::None);
        assert_eq!(match_uuid_prefix(&ids, "  "), PrefixMatch::None);
    }

    #[test]
    fn uuid_prefix_case_insensitive() {
        let ids = vec![uid(0xabcd_ef00_0000_0000_0000_0000_0000_0000)];
        assert_eq!(match_uuid_prefix(&ids, "ABCDEF"), PrefixMatch::Unique(0));
    }

    // -- match_entity -------------------------------------------

    fn rows() -> Vec<Candidate> {
        vec![
            (
                uid(0xa1),
                "The Band Mosaic".into(),
                "Projects/The Band Mosaic/The Band Mosaic.md".into(),
            ),
            (uid(0xa2), "Task".into(), "Projects/Task/Task.md".into()),
            (
                uid(0xb3),
                "Task Website".into(),
                "Projects/Task Website/Task Website.md".into(),
            ),
        ]
    }

    #[test]
    fn project_exact_path_wins() {
        assert_eq!(
            match_entity(&rows(), "Projects/Task/Task.md", "project"),
            Ok(1)
        );
    }

    #[test]
    fn project_exact_title_case_insensitive() {
        // "Task" is also a title-prefix of "Task Website" — exact
        // title must win before the prefix level sees it.
        assert_eq!(match_entity(&rows(), "task", "project"), Ok(1));
        assert_eq!(match_entity(&rows(), "the band mosaic", "project"), Ok(0));
    }

    #[test]
    fn project_unique_title_prefix() {
        assert_eq!(match_entity(&rows(), "The Band", "project"), Ok(0));
        assert_eq!(match_entity(&rows(), "task w", "project"), Ok(2));
    }

    #[test]
    fn project_ambiguous_title_prefix_errors() {
        let mut r = rows();
        r.push((
            uid(0xc4),
            "The Bandsaw".into(),
            "Projects/Bandsaw.md".into(),
        ));
        match match_entity(&r, "The Band", "project").unwrap_err() {
            MatchFailure::Ambiguous(msg) => assert!(msg.contains("ambiguous"), "got: {msg}"),
            other @ MatchFailure::NotFound(_) => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn project_id_prefix_fallback() {
        // Prefixes compare against the hyphenated `Uuid::to_string()`
        // form — same convention as `resolve_issue_id` (short
        // prefixes never reach a hyphen; long ones must include them).
        assert_eq!(
            match_entity(&rows(), "00000000-0000-0000-0000-0000000000b3", "project"),
            Ok(2)
        );
        // All three ids share the "00000000" head → ambiguous.
        assert!(match_entity(&rows(), "00000000", "project").is_err());
    }

    #[test]
    fn project_no_match_errors() {
        match match_entity(&rows(), "zzz", "project").unwrap_err() {
            MatchFailure::NotFound(msg) => assert!(msg.contains("no project"), "got: {msg}"),
            other @ MatchFailure::Ambiguous(_) => panic!("expected NotFound, got {other:?}"),
        }
        assert!(match_entity(&rows(), "", "project").is_err());
    }

    #[test]
    fn entity_noun_lands_in_messages() {
        // Same matcher serves goals / milestones / orgs — the noun
        // parameter is the only difference.
        let r = vec![(
            uid(0x11),
            "Fasttrack Studios".into(),
            "fasttrackstudios".into(),
        )];
        match match_entity(&r, "zzz", "organization").unwrap_err() {
            MatchFailure::NotFound(msg) => {
                assert!(msg.contains("no organization"), "got: {msg}");
            }
            other @ MatchFailure::Ambiguous(_) => panic!("expected NotFound, got {other:?}"),
        }
        // Slug (the key slot) exact-matches at level 1.
        assert_eq!(match_entity(&r, "fasttrackstudios", "organization"), Ok(0));
        // Name prefix matches at level 3.
        assert_eq!(match_entity(&r, "fasttrack stu", "organization"), Ok(0));
    }

    #[test]
    fn match_failure_maps_to_taxonomy() {
        use crate::errors;
        let nf = MatchFailure::NotFound("nope".into()).into_report("task", "zzz");
        assert_eq!(errors::exit_code(&nf), 4);
        let amb = MatchFailure::Ambiguous("many".into()).into_report("task", "00");
        assert_eq!(errors::exit_code(&amb), 5);
    }

    // -- session_json -------------------------------------------

    fn sample_session() -> timer_proto::WorkSession {
        let start = chrono::Utc::now() - chrono::Duration::seconds(90);
        timer_proto::WorkSession {
            id: uid(7),
            org_id: uid(8),
            user_id: uid(9),
            project_id: Some(uid(10)),
            project_path: "Projects/Task/Task.md".into(),
            description: "smoke".into(),
            start_time: start,
            end_time: Some(start + chrono::Duration::seconds(60)),
            billable: true,
            rate_cents: 3000,
            currency: "USD".into(),
            task_note_path: "Task/foo.md".into(),
            invoice_id: None,
            created_at: start,
            updated_at: start,
        }
    }

    #[test]
    fn session_json_adds_seconds_elapsed() {
        let s = sample_session();
        let v = session_json(&s);
        assert_eq!(v["seconds_elapsed"], 60);
        assert_eq!(v["description"], "smoke");
        assert_eq!(v["rate_cents"], 3000);
    }

    #[test]
    fn open_session_elapsed_uses_now() {
        let mut s = sample_session();
        s.end_time = None;
        let secs = session_seconds(&s);
        assert!((85..=120).contains(&secs), "got {secs}");
    }

    #[test]
    fn joined_titles_only_when_resolved() {
        let s = sample_session();
        let v = session_json_joined(&s, Some("Fix the thing".into()), None);
        assert_eq!(v["task_title"], "Fix the thing");
        assert!(v.get("project_title").is_none());
    }
}
