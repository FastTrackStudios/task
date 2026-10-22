//! A live set's song docs, read and edited as JSON (`live_observe`,
//! `doc_read`, `doc_apply` over MCP): how an agent works in a live set
//! without speaking Loro.
//!
//! A song's doc in a live set ([`crate::live`]) is a Loro doc that the
//! band's apps keep in sync over `DocSync`. Here it is read as JSON plus a
//! **version** (the doc's frontiers, `counter@peer,…`; `empty` for a doc
//! nobody has written), and written with a short list of JSON **ops**:
//!
//! | op | does |
//! |---|---|
//! | `{"op":"set","path":[…,key],"value":v}` | set a map key (or replace a list item) |
//! | `{"op":"insert","path":[…list],"index":i,"value":v}` | insert into a list (`index` omitted: append) |
//! | `{"op":"delete","path":[…,key or index]}` | delete a map key or a list item |
//! | `{"op":"splice","path":[…text],"index":i,"delete":n,"insert":s}` | edit a text |
//! | `{"op":"undo","apply":"<id>"}` | revert an earlier apply (alone in its batch) |
//!
//! A path starts at one of the doc's roots (`["song","tempo"]`); a
//! string path splits on `/` (`"song/sections/0/name"`). Objects and
//! arrays in a value become maps and lists, so a peer edits them field
//! by field like anything else.
//!
//! **The rules** (spec §7 S3, with the Captain's §11 Q2: edits apply by
//! default, a proposal is the opt-in):
//!
//! - **Schema check.** Every op is checked against the doc's current
//!   shape before anything changes: the path must resolve, a list index
//!   must exist, a text is spliced within its length, and a `set` keeps a
//!   field's kind — a number stays a number, a list stays a list (`null`
//!   on either side is the only way to change kind: set it to `null`, or
//!   delete it, first). The whole batch is tried on a fork of the doc;
//!   one bad op refuses the batch and nothing changes.
//! - **`expected_version`.** When given, the doc must still be at that
//!   version, or the batch is refused as stale: read again, then re-apply.
//! - **Attribution.** Each apply is one commit whose message names the
//!   caller (`created_by`, its name, and `on_behalf_of` when the caller
//!   is an assistant acting for someone), so the CRDT history every peer
//!   holds says who made it. The ledger here keeps the same, newest last,
//!   for `doc_read`.
//! - **Undo.** An apply's id undoes it: the inverse is computed by Loro's
//!   undo manager, transformed over whatever peers did since, and goes
//!   out to every peer like any edit. Undo runs newest first — an older
//!   apply is undone after the ones made over it.
//! - **`mode: "propose"`** runs the same checks on a fork and answers with
//!   what the doc would become, changing nothing.
//!
//! Who may do this is the MCP lane's business (`crate::mcp`): reading
//! needs `read` on `live/<setlist>`, applying needs `write` (org roles
//! and grants), and an observer — a key that only watches — is refused.

use std::collections::HashMap;
use std::sync::Mutex;

use crdt::loro::{
    Container, Index, LoroDoc, LoroList, LoroMap, LoroValue, UndoManager, ValueOrContainer,
};
use live_proto::{LiveError, LiveSet};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::live::LiveHost;

/// The most ops one apply takes.
pub const MAX_OPS: usize = 256;
/// How deep a value may nest.
const MAX_DEPTH: usize = 32;
/// How many applies `doc_read` shows, newest last.
const HISTORY_SHOWN: usize = 20;
/// The ledger keeps at most this many applies per doc (Loro's undo
/// manager keeps 100 steps; nothing older can be undone anyway).
const LEDGER_KEPT: usize = 100;
/// What a doc ref of a live song starts with.
const LIVE_REF: &str = "live:";

/// Who an apply is from: the caller, as the MCP lane authenticated it —
/// never from the tool's arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attribution {
    /// `user:<uuid>` / `bot:<uuid>`; `None` for the server itself.
    pub created_by: Option<String>,
    pub created_by_name: Option<String>,
    /// Whom an assistant acted for, when its key says so.
    pub on_behalf_of: Option<String>,
}

impl Attribution {
    fn json(&self) -> Value {
        json!({
            "created_by": self.created_by,
            "created_by_name": self.created_by_name,
            "on_behalf_of": self.on_behalf_of,
        })
    }
}

/// One apply, as the ledger keeps it.
#[derive(Debug, Clone)]
struct Applied {
    id: String,
    by: Attribution,
    at: String,
    ops: usize,
    before: String,
    after: String,
    /// The apply this one undid, for an undo.
    undoes: Option<String>,
    /// The undo that reverted this one.
    undone_by: Option<String>,
}

impl Applied {
    fn json(&self) -> Value {
        let mut v = self.by.json();
        let o = v.as_object_mut().expect("an object");
        o.insert("apply".into(), json!(self.id));
        o.insert("at".into(), json!(self.at));
        o.insert("ops".into(), json!(self.ops));
        o.insert("version_before".into(), json!(self.before));
        o.insert("version_after".into(), json!(self.after));
        o.insert("undoes".into(), json!(self.undoes));
        o.insert("undone_by".into(), json!(self.undone_by));
        v
    }
}

/// One served doc's applies and the undo manager that can revert them.
struct DocEdits {
    /// The served doc's peer when this began: a doc let go and opened
    /// again is a new doc (a new peer), and its ledger starts over.
    peer: u64,
    undo: UndoManager,
    applies: Vec<Applied>,
}

/// Every live doc's applies, for one org's [`LiveHost`]. Applies are
/// serialized here — a version check and the edit it guards are one
/// step.
#[derive(Default)]
pub struct Edits {
    docs: Mutex<HashMap<Uuid, DocEdits>>,
}

impl Edits {
    /// Let go of docs no longer served (a new epoch).
    pub fn forget(&self, ids: &[Uuid]) {
        let mut docs = lock(&self.docs);
        for id in ids {
            docs.remove(id);
        }
    }

    fn history(&self, doc_id: Uuid, peer: u64) -> Vec<Value> {
        let docs = lock(&self.docs);
        docs.get(&doc_id)
            .filter(|d| d.peer == peer)
            .map(|d| {
                let skip = d.applies.len().saturating_sub(HISTORY_SHOWN);
                d.applies.iter().skip(skip).map(Applied::json).collect()
            })
            .unwrap_or_default()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ── Doc refs ─────────────────────────────────────────────────────

/// A song's doc ref: `live:<setlist>/<slug>`.
#[must_use]
pub fn doc_ref(setlist: &str, slug: &str) -> String {
    format!("{LIVE_REF}{setlist}/{slug}")
}

/// A served song doc, found by its ref.
struct Song {
    setlist: String,
    slug: String,
    doc_id: Uuid,
}

/// `live:<setlist>/<slug>`, or a song doc's bare id from a set.
fn resolve(host: &LiveHost, raw: &str) -> Result<Song, String> {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix(LIVE_REF) {
        let (setlist, slug) = rest.rsplit_once('/').ok_or_else(|| {
            format!(
                "`{raw}` is not a doc ref: write live:<setlist>/<song> (live_observe lists them)"
            )
        })?;
        let set = host.join(setlist, None).map_err(live_err)?;
        let song = set.songs.iter().find(|s| s.slug == slug).ok_or_else(|| {
            format!(
                "set `{setlist}` has no song `{slug}`; its songs: {}",
                set.songs
                    .iter()
                    .map(|s| s.slug.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        let doc_id = song
            .doc_id
            .parse()
            .map_err(|e| format!("a bad doc id in the set: {e}"))?;
        return Ok(Song {
            setlist: setlist.to_owned(),
            slug: slug.to_owned(),
            doc_id,
        });
    }
    let doc_id: Uuid = raw.parse().map_err(|_| {
        format!("`{raw}` is not a doc ref: write live:<setlist>/<song> (live_observe lists them)")
    })?;
    let (setlist, slug) = host
        .locate(doc_id)
        .ok_or_else(|| format!("doc `{doc_id}` is not a song of a live set served now"))?;
    Ok(Song {
        setlist,
        slug,
        doc_id,
    })
}

/// The setlist a ref names, without opening anything — what the
/// permission check is made against before the doc is touched.
///
/// # Errors
///
/// Not a doc ref.
pub fn setlist_of(host: &LiveHost, raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix(LIVE_REF) {
        return rest
            .rsplit_once('/')
            .map(|(setlist, _)| setlist.to_owned())
            .ok_or_else(|| format!("`{raw}` is not a doc ref: write live:<setlist>/<song>"));
    }
    let doc_id: Uuid = raw
        .parse()
        .map_err(|_| format!("`{raw}` is not a doc ref: write live:<setlist>/<song>"))?;
    host.locate(doc_id)
        .map(|(setlist, _)| setlist)
        .ok_or_else(|| format!("doc `{doc_id}` is not a song of a live set served now"))
}

fn live_err(e: LiveError) -> String {
    match e {
        LiveError::NotFound(s) => {
            format!("no setlist `{s}`: live sets are the org's song lists and set lists")
        }
        other => other.to_string(),
    }
}

/// The served doc for `doc_id`. Runs on the blocking pool (MCP tools
/// do), where blocking on the registry is allowed.
fn served(host: &LiveHost, doc_id: Uuid) -> Result<crdt::CrdtDoc, String> {
    tokio::runtime::Handle::current()
        .block_on(host.registry().doc(doc_id))
        .map_err(|e| format!("the doc is not served: {e}"))
}

// ── The tools ────────────────────────────────────────────────────

// t[impl mcp.live.observe]
/// `live_observe`: a set as an agent watches it — its songs, each with
/// its doc ref and current version. Opens the set if nobody is in it.
///
/// # Errors
///
/// No such setlist.
pub fn observe(host: &LiveHost, setlist: &str, can_apply: bool) -> Result<Value, String> {
    let set: LiveSet = host.join(setlist, None).map_err(live_err)?;
    let mut songs = Vec::with_capacity(set.songs.len());
    for song in &set.songs {
        let id: Uuid = song
            .doc_id
            .parse()
            .map_err(|e| format!("a bad doc id in the set: {e}"))?;
        let doc = served(host, id)?;
        songs.push(json!({
            "slug": song.slug,
            "title": song.title,
            "doc_ref": doc_ref(&set.setlist, &song.slug),
            "doc_id": song.doc_id,
            "version": version(doc.loro()),
        }));
    }
    let note = if can_apply {
        "doc_read a song's doc_ref for its JSON and version; doc_apply edits it live \
         (pass expected_version to refuse a stale edit)."
    } else {
        "You watch this set: doc_read works, doc_apply is refused."
    };
    Ok(json!({
        "setlist": set.setlist,
        "title": set.title,
        "epoch": set.epoch,
        "presence_id": set.presence_id,
        "resets_at": set.resets_at,
        "read_only": !can_apply,
        "songs": songs,
        "note": note,
    }))
}

/// `doc_read`: a song's doc as JSON, its version, and the latest applies.
///
/// # Errors
///
/// A bad ref, or the doc is not served.
pub fn read(host: &LiveHost, raw_ref: &str) -> Result<Value, String> {
    let song = resolve(host, raw_ref)?;
    let doc = served(host, song.doc_id)?;
    let loro = doc.loro();
    Ok(json!({
        "doc_ref": doc_ref(&song.setlist, &song.slug),
        "doc_id": song.doc_id.to_string(),
        "setlist": song.setlist,
        "song": song.slug,
        "version": version(loro),
        "value": to_json(&loro.get_deep_value()),
        "applies": host.edits().history(song.doc_id, loro.peer_id()),
    }))
}

/// What one `doc_apply` asks.
pub struct ApplyRequest<'a> {
    pub ops: &'a Value,
    pub expected_version: Option<&'a str>,
    /// Check and preview only (`mode: "propose"`).
    pub propose: bool,
    pub by: Attribution,
}

// t[impl mcp.live.apply]
// t[impl mcp.live.expected-version]
// t[impl mcp.live.attribution]
/// `doc_apply`: check `ops` against the doc and apply them (or, for a
/// proposal, show what they would make), as one attributed commit.
///
/// # Errors
///
/// A bad ref, a malformed or failing op, a stale `expected_version`, an
/// undo that cannot run.
pub fn apply(host: &LiveHost, raw_ref: &str, req: &ApplyRequest<'_>) -> Result<Value, String> {
    let ops = parse_ops(req.ops)?;
    let song = resolve(host, raw_ref)?;
    let doc = served(host, song.doc_id)?;
    let loro = doc.loro();
    let this_ref = doc_ref(&song.setlist, &song.slug);

    // Serialized: the version check and the edit it guards are one step.
    let edits = host.edits();
    let mut docs = lock(&edits.docs);
    let before = version(loro);
    if let Some(expected) = req.expected_version
        && expected.trim() != before
    {
        return Err(format!(
            "stale: `{this_ref}` is at version {before}, not {}. Nothing was applied — \
             doc_read it again and re-apply against what is there now.",
            expected.trim()
        ));
    }

    if let [Op::Undo { apply }] = ops.as_slice() {
        if req.propose {
            return Err("an undo can't be proposed: apply it, or leave it".into());
        }
        return undo(
            &mut docs,
            song.doc_id,
            loro,
            apply.as_deref(),
            &req.by,
            &this_ref,
        );
    }
    if ops.iter().any(|op| matches!(op, Op::Undo { .. })) {
        return Err("an undo goes alone in its doc_apply".into());
    }

    // The schema check: the whole batch on a fork first. One bad op
    // refuses the batch, and the served doc is untouched.
    let fork = loro.fork();
    for (i, op) in ops.iter().enumerate() {
        run(&fork, op).map_err(|e| format!("op {i}: {e}. Nothing was applied."))?;
    }
    fork.commit();
    if req.propose {
        return Ok(json!({
            "doc_ref": this_ref,
            "mode": "propose",
            "applied": false,
            "version": before,
            "ops": ops.len(),
            "would_be": to_json(&fork.get_deep_value()),
            "note": "Checked, not applied. doc_apply the same ops (with expected_version) to make it so.",
        }));
    }

    let entry = edits_for(&mut docs, song.doc_id, loro);
    let id = Uuid::new_v4().simple().to_string();
    let mut failed = None;
    for (i, op) in ops.iter().enumerate() {
        // A peer's edit can land between the check and here; whatever
        // was applied stays one undoable apply.
        if let Err(e) = run(loro, op) {
            failed = Some(format!("op {i}: {e}"));
            break;
        }
    }
    loro.set_next_commit_message(&commit_message(&id, &req.by));
    loro.commit();
    let _ = entry.undo.record_new_checkpoint();
    let after = version(loro);
    remember(
        entry,
        Applied {
            id: id.clone(),
            by: req.by.clone(),
            at: chrono::Utc::now().to_rfc3339(),
            ops: ops.len(),
            before: before.clone(),
            after: after.clone(),
            undoes: None,
            undone_by: None,
        },
    );
    if let Some(failed) = failed {
        return Err(format!(
            "{failed} — a peer changed the doc while this applied. What applied before it is \
             apply `{id}`; doc_apply [{{\"op\":\"undo\",\"apply\":\"{id}\"}}] reverts it."
        ));
    }
    Ok(json!({
        "doc_ref": this_ref,
        "mode": "apply",
        "applied": true,
        "apply": id,
        "ops": ops.len(),
        "version_before": before,
        "version": after,
        "attribution": req.by.json(),
        "undo": [{ "op": "undo", "apply": id }],
    }))
}

/// This doc's ledger, begun (with its undo manager) on the first apply
/// — or begun again when the served doc is a new one.
fn edits_for<'a>(
    docs: &'a mut HashMap<Uuid, DocEdits>,
    doc_id: Uuid,
    loro: &LoroDoc,
) -> &'a mut DocEdits {
    let peer = loro.peer_id();
    let fresh = docs.get(&doc_id).is_none_or(|d| d.peer != peer);
    if fresh {
        let mut undo = UndoManager::new(loro);
        // One apply, one step: never merged with the next.
        undo.set_merge_interval(0);
        undo.set_max_undo_steps(LEDGER_KEPT);
        docs.insert(
            doc_id,
            DocEdits {
                peer,
                undo,
                applies: Vec::new(),
            },
        );
    }
    docs.get_mut(&doc_id).expect("just ensured")
}

fn remember(entry: &mut DocEdits, applied: Applied) {
    entry.applies.push(applied);
    let over = entry.applies.len().saturating_sub(LEDGER_KEPT);
    entry.applies.drain(..over);
}

// t[impl mcp.live.undo]
/// Revert an apply (the newest not yet undone when `apply` is `None`).
fn undo(
    docs: &mut HashMap<Uuid, DocEdits>,
    doc_id: Uuid,
    loro: &LoroDoc,
    apply: Option<&str>,
    by: &Attribution,
    doc_ref: &str,
) -> Result<Value, String> {
    let peer = loro.peer_id();
    let Some(entry) = docs.get_mut(&doc_id).filter(|d| d.peer == peer) else {
        return Err(format!("`{doc_ref}` has no applies to undo"));
    };
    // The undo stack holds applies, never undos: the newest apply not
    // yet undone is the top of the stack.
    let Some(top) = entry
        .applies
        .iter()
        .rposition(|a| a.undoes.is_none() && a.undone_by.is_none())
    else {
        return Err(format!("`{doc_ref}` has no applies left to undo"));
    };
    let target = entry.applies[top].id.clone();
    if let Some(wanted) = apply.map(str::trim)
        && wanted != target
    {
        return Err(match entry.applies.iter().find(|a| a.id == wanted) {
            Some(a) if a.undone_by.is_some() => format!("apply `{wanted}` is already undone"),
            Some(a) if a.undoes.is_some() => {
                format!("`{wanted}` is an undo; apply its ops again instead")
            }
            Some(_) => format!(
                "apply `{wanted}` has later applies over it; undo `{target}` first (undo runs \
                 newest first)"
            ),
            None => format!("no apply `{wanted}` on `{doc_ref}` (doc_read lists them)"),
        });
    }
    if !entry.undo.can_undo() {
        return Err(format!(
            "apply `{target}` is too old to undo (the doc keeps {LEDGER_KEPT} steps)"
        ));
    }
    let before = version(loro);
    let undone = entry
        .undo
        .undo()
        .map_err(|e| format!("couldn't undo `{target}`: {e}"))?;
    if !undone {
        return Err(format!("apply `{target}` had nothing left to undo"));
    }
    loro.commit();
    let after = version(loro);
    let id = Uuid::new_v4().simple().to_string();
    entry.applies[top].undone_by = Some(id.clone());
    remember(
        entry,
        Applied {
            id: id.clone(),
            by: by.clone(),
            at: chrono::Utc::now().to_rfc3339(),
            ops: 1,
            before: before.clone(),
            after: after.clone(),
            undoes: Some(target.clone()),
            undone_by: None,
        },
    );
    Ok(json!({
        "doc_ref": doc_ref,
        "mode": "apply",
        "applied": true,
        "apply": id,
        "undid": target,
        "version_before": before,
        "version": after,
        "attribution": by.json(),
    }))
}

/// The commit message an apply carries into the CRDT history.
fn commit_message(id: &str, by: &Attribution) -> String {
    json!({
        "via": "mcp:doc_apply",
        "apply": id,
        "created_by": by.created_by,
        "created_by_name": by.created_by_name,
        "on_behalf_of": by.on_behalf_of,
    })
    .to_string()
}

/// A doc's version: its frontiers, `counter@peer` sorted and joined;
/// `empty` before anyone wrote.
#[must_use]
pub fn version(doc: &LoroDoc) -> String {
    let mut ids: Vec<String> = doc
        .oplog_frontiers()
        .iter()
        .map(|id| format!("{}@{}", id.counter, id.peer))
        .collect();
    if ids.is_empty() {
        return "empty".into();
    }
    ids.sort();
    ids.join(",")
}

// ── Ops ──────────────────────────────────────────────────────────

/// One edit.
#[derive(Debug, Clone, PartialEq)]
enum Op {
    Set {
        path: Vec<String>,
        value: Value,
    },
    Insert {
        path: Vec<String>,
        index: Option<usize>,
        value: Value,
    },
    Delete {
        path: Vec<String>,
    },
    Splice {
        path: Vec<String>,
        index: usize,
        delete: usize,
        insert: String,
    },
    Undo {
        apply: Option<String>,
    },
}

fn parse_ops(raw: &Value) -> Result<Vec<Op>, String> {
    let list = raw.as_array().ok_or(
        "`ops` is a list of ops: [{\"op\":\"set\",\"path\":[\"song\",\"tempo\"],\"value\":78}]",
    )?;
    if list.is_empty() {
        return Err("`ops` is empty: nothing to apply".into());
    }
    if list.len() > MAX_OPS {
        return Err(format!(
            "{} ops in one apply; at most {MAX_OPS}",
            list.len()
        ));
    }
    list.iter()
        .enumerate()
        .map(|(i, v)| parse_op(v).map_err(|e| format!("op {i}: {e}")))
        .collect()
}

fn parse_op(v: &Value) -> Result<Op, String> {
    let kind = v
        .get("op")
        .and_then(Value::as_str)
        .ok_or("no `op` (set, insert, delete, splice or undo)")?;
    let value = |v: &Value| -> Result<Value, String> {
        let value = v.get("value").cloned().ok_or("no `value`")?;
        if depth(&value) > MAX_DEPTH {
            return Err(format!("`value` nests deeper than {MAX_DEPTH}"));
        }
        Ok(value)
    };
    let usize_at = |key: &str| -> Result<Option<usize>, String> {
        match v.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(n) => n
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .map(Some)
                .ok_or_else(|| format!("`{key}` is a whole number ≥ 0")),
        }
    };
    match kind {
        "set" => Ok(Op::Set {
            path: parse_path(v.get("path"))?,
            value: value(v)?,
        }),
        "insert" => Ok(Op::Insert {
            path: parse_path(v.get("path"))?,
            index: usize_at("index")?,
            value: value(v)?,
        }),
        "delete" => Ok(Op::Delete {
            path: parse_path(v.get("path"))?,
        }),
        "splice" => Ok(Op::Splice {
            path: parse_path(v.get("path"))?,
            index: usize_at("index")?.ok_or("no `index`")?,
            delete: usize_at("delete")?.unwrap_or(0),
            insert: match v.get("insert") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(_) => return Err("`insert` is a string".into()),
            },
        }),
        "undo" => Ok(Op::Undo {
            apply: v.get("apply").and_then(Value::as_str).map(str::to_owned),
        }),
        other => Err(format!(
            "unknown op `{other}` (set, insert, delete, splice or undo)"
        )),
    }
}

fn parse_path(v: Option<&Value>) -> Result<Vec<String>, String> {
    let segs: Vec<String> = match v {
        Some(Value::String(s)) => s
            .trim()
            .trim_start_matches('/')
            .split('/')
            .map(str::to_owned)
            .collect(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|seg| match seg {
                Value::String(s) => Ok(s.clone()),
                Value::Number(n) if n.as_u64().is_some() => Ok(n.to_string()),
                other => Err(format!("a path segment is a key or an index, not {other}")),
            })
            .collect::<Result<_, _>>()?,
        _ => return Err("no `path` (e.g. [\"song\",\"tempo\"] or \"song/tempo\")".into()),
    };
    if segs.is_empty() || segs.iter().any(String::is_empty) {
        return Err("`path` has an empty segment".into());
    }
    Ok(segs)
}

fn depth(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

fn shown(path: &[String]) -> String {
    format!("/{}", path.join("/"))
}

/// Apply one op to `doc` (uncommitted), checking it against the doc's
/// shape first.
fn run(doc: &LoroDoc, op: &Op) -> Result<(), String> {
    match op {
        Op::Set { path, value } => set(doc, path, value),
        Op::Insert { path, index, value } => insert(doc, path, *index, value),
        Op::Delete { path } => delete(doc, path),
        Op::Splice {
            path,
            index,
            delete,
            insert,
        } => splice(doc, path, *index, *delete, insert),
        Op::Undo { .. } => Err("an undo goes alone in its doc_apply".into()),
    }
}

/// The root container `name`, if the doc has one.
fn root(doc: &LoroDoc, name: &str) -> Option<Container> {
    match doc.get_by_path(&[Index::Key(name.into())]) {
        Some(ValueOrContainer::Container(c)) => Some(c),
        _ => None,
    }
}

/// The container at `path` (a root, or a container inside one).
fn container_at(doc: &LoroDoc, path: &[String]) -> Result<Container, String> {
    let (first, rest) = path.split_first().ok_or("an empty path")?;
    let mut here = root(doc, first).ok_or_else(|| format!("the doc has no `{first}`"))?;
    for (i, seg) in rest.iter().enumerate() {
        let at = &path[..i + 2];
        let next = child(&here, seg, at)?.ok_or_else(|| format!("nothing at {}", shown(at)))?;
        here = match next {
            ValueOrContainer::Container(c) => c,
            ValueOrContainer::Value(_) => {
                return Err(format!("{} is a value, not a map, list or text", shown(at)));
            }
        };
    }
    Ok(here)
}

/// `seg` inside `parent`.
fn child(parent: &Container, seg: &str, at: &[String]) -> Result<Option<ValueOrContainer>, String> {
    match parent {
        Container::Map(m) => Ok(m.get(seg)),
        Container::List(l) => Ok(l.get(list_index(seg, l.len(), at, false)?)),
        Container::MovableList(l) => Ok(l.get(list_index(seg, l.len(), at, false)?)),
        Container::Text(_) => Err(format!(
            "{} is inside a text; edit a text with splice",
            shown(at)
        )),
        _ => Err(format!(
            "{} is in a container doc_apply doesn't edit",
            shown(at)
        )),
    }
}

/// A list index from a path segment, which must exist (or, with
/// `end_ok`, be one past the end).
fn list_index(seg: &str, len: usize, at: &[String], end_ok: bool) -> Result<usize, String> {
    let i: usize = seg
        .parse()
        .map_err(|_| format!("{} indexes a list: use a number", shown(at)))?;
    if i < len || (end_ok && i == len) {
        Ok(i)
    } else {
        Err(format!(
            "{}: index {i} is past the end of a list of {len}",
            shown(at)
        ))
    }
}

/// What kind of thing a doc holds, as the schema check names it.
fn kind_of(v: &ValueOrContainer) -> &'static str {
    match v {
        ValueOrContainer::Value(v) => match v {
            LoroValue::Null => "null",
            LoroValue::Bool(_) => "boolean",
            LoroValue::Double(_) | LoroValue::I64(_) => "number",
            LoroValue::String(_) => "string",
            LoroValue::Binary(_) => "binary",
            LoroValue::List(_) => "array",
            LoroValue::Map(_) => "object",
            LoroValue::Container(_) => "container",
        },
        ValueOrContainer::Container(c) => match c {
            Container::Map(_) => "object",
            Container::List(_) | Container::MovableList(_) => "array",
            Container::Text(_) => "text",
            Container::Tree(_) => "tree",
            _ => "container",
        },
    }
}

/// What kind a JSON value is.
fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The schema check for a `set`: the field keeps its kind. `null` on
/// either side changes it; a string may replace a text's content.
fn same_kind(old: &ValueOrContainer, new: &Value, at: &[String]) -> Result<(), String> {
    let (was, is) = (kind_of(old), json_kind(new));
    if was == is || was == "null" || is == "null" || (was == "text" && is == "string") {
        return Ok(());
    }
    if matches!(was, "binary" | "tree" | "container") {
        return Err(format!(
            "{} is a {was}; doc_apply doesn't edit it",
            shown(at)
        ));
    }
    Err(format!(
        "schema: {} is a {was}, not a {is}. A set keeps a field's kind — set it to null or \
         delete it first to change what it is",
        shown(at)
    ))
}

fn set(doc: &LoroDoc, path: &[String], value: &Value) -> Result<(), String> {
    let Some((key, parent_path)) = path.split_last() else {
        return Err("an empty path".into());
    };
    if parent_path.is_empty() {
        return Err(format!(
            "`{key}` is a root of the doc; set a field inside it (path [\"{key}\", …])"
        ));
    }
    // A root map that doesn't exist yet is made by its first field.
    let parent = if parent_path.len() == 1 && root(doc, &parent_path[0]).is_none() {
        Container::Map(doc.get_map(parent_path[0].as_str()))
    } else {
        container_at(doc, parent_path)?
    };
    match parent {
        Container::Map(m) => {
            if let Some(old) = m.get(key) {
                same_kind(&old, value, path)?;
                if let (ValueOrContainer::Container(Container::Text(t)), Value::String(s)) =
                    (&old, value)
                {
                    return t
                        .splice(0, t.len_unicode(), s)
                        .map(drop)
                        .map_err(|e| e.to_string());
                }
            }
            write_map(&m, key, value)
        }
        Container::List(l) => {
            let i = list_index(key, l.len(), path, false)?;
            if let Some(old) = l.get(i) {
                same_kind(&old, value, path)?;
            }
            l.delete(i, 1).map_err(|e| e.to_string())?;
            write_list(&l, i, value)
        }
        Container::MovableList(l) => {
            let i = list_index(key, l.len(), path, false)?;
            if let Some(old) = l.get(i) {
                same_kind(&old, value, path)?;
            }
            match value {
                Value::Object(_) => {
                    let m = l
                        .set_container(i, LoroMap::new())
                        .map_err(|e| e.to_string())?;
                    fill_map(&m, value)
                }
                Value::Array(_) => {
                    let child = l
                        .set_container(i, LoroList::new())
                        .map_err(|e| e.to_string())?;
                    fill_list(&child, value)
                }
                scalar => l.set(i, scalar_of(scalar)).map_err(|e| e.to_string()),
            }
        }
        Container::Text(_) => Err(format!(
            "{} is a text; edit it with splice",
            shown(parent_path)
        )),
        _ => Err(format!(
            "{} is a container doc_apply doesn't edit",
            shown(parent_path)
        )),
    }
}

fn insert(
    doc: &LoroDoc,
    path: &[String],
    index: Option<usize>,
    value: &Value,
) -> Result<(), String> {
    // A root list that doesn't exist yet is made by its first item.
    let target = if path.len() == 1 && root(doc, &path[0]).is_none() {
        Container::List(doc.get_list(path[0].as_str()))
    } else {
        container_at(doc, path)?
    };
    let check = |len: usize| -> Result<usize, String> {
        let i = index.unwrap_or(len);
        if i > len {
            return Err(format!(
                "{}: index {i} is past the end of a list of {len}",
                shown(path)
            ));
        }
        Ok(i)
    };
    match target {
        Container::List(l) => {
            let i = check(l.len())?;
            write_list(&l, i, value)
        }
        Container::MovableList(l) => {
            let i = check(l.len())?;
            match value {
                Value::Object(_) => {
                    let m = l
                        .insert_container(i, LoroMap::new())
                        .map_err(|e| e.to_string())?;
                    fill_map(&m, value)
                }
                Value::Array(_) => {
                    let child = l
                        .insert_container(i, LoroList::new())
                        .map_err(|e| e.to_string())?;
                    fill_list(&child, value)
                }
                scalar => l.insert(i, scalar_of(scalar)).map_err(|e| e.to_string()),
            }
        }
        other => Err(format!(
            "schema: {} is a {}, not a list; insert goes into a list",
            shown(path),
            kind_of(&ValueOrContainer::Container(other))
        )),
    }
}

fn delete(doc: &LoroDoc, path: &[String]) -> Result<(), String> {
    let Some((key, parent_path)) = path.split_last() else {
        return Err("an empty path".into());
    };
    if parent_path.is_empty() {
        return Err(format!(
            "`{key}` is a root of the doc and stays; delete what is inside it"
        ));
    }
    match container_at(doc, parent_path)? {
        Container::Map(m) => {
            if m.get(key).is_none() {
                return Err(format!("nothing at {}", shown(path)));
            }
            m.delete(key).map_err(|e| e.to_string())
        }
        Container::List(l) => {
            let i = list_index(key, l.len(), path, false)?;
            l.delete(i, 1).map_err(|e| e.to_string())
        }
        Container::MovableList(l) => {
            let i = list_index(key, l.len(), path, false)?;
            l.delete(i, 1).map_err(|e| e.to_string())
        }
        Container::Text(_) => Err(format!(
            "{} is a text; edit it with splice",
            shown(parent_path)
        )),
        _ => Err(format!(
            "{} is a container doc_apply doesn't edit",
            shown(parent_path)
        )),
    }
}

fn splice(
    doc: &LoroDoc,
    path: &[String],
    index: usize,
    del: usize,
    ins: &str,
) -> Result<(), String> {
    let Container::Text(t) = container_at(doc, path)? else {
        return Err(format!(
            "schema: {} is not a text; splice edits a text",
            shown(path)
        ));
    };
    let len = t.len_unicode();
    if index > len || del > len - index {
        return Err(format!(
            "{}: splice {index}+{del} runs past a text of {len} characters",
            shown(path)
        ));
    }
    t.splice(index, del, ins)
        .map(drop)
        .map_err(|e| e.to_string())
}

fn scalar_of(v: &Value) -> LoroValue {
    match v {
        Value::Bool(b) => LoroValue::from(*b),
        Value::Number(n) => n.as_i64().map_or_else(
            || LoroValue::from(n.as_f64().unwrap_or(0.0)),
            LoroValue::from,
        ),
        Value::String(s) => LoroValue::from(s.as_str()),
        // Null; objects and arrays are written as containers by the
        // callers, never through here.
        _ => LoroValue::Null,
    }
}

fn write_map(m: &LoroMap, key: &str, value: &Value) -> Result<(), String> {
    match value {
        Value::Object(_) => {
            let child = m
                .insert_container(key, LoroMap::new())
                .map_err(|e| e.to_string())?;
            fill_map(&child, value)
        }
        Value::Array(_) => {
            let child = m
                .insert_container(key, LoroList::new())
                .map_err(|e| e.to_string())?;
            fill_list(&child, value)
        }
        scalar => m.insert(key, scalar_of(scalar)).map_err(|e| e.to_string()),
    }
}

fn write_list(l: &LoroList, i: usize, value: &Value) -> Result<(), String> {
    match value {
        Value::Object(_) => {
            let child = l
                .insert_container(i, LoroMap::new())
                .map_err(|e| e.to_string())?;
            fill_map(&child, value)
        }
        Value::Array(_) => {
            let child = l
                .insert_container(i, LoroList::new())
                .map_err(|e| e.to_string())?;
            fill_list(&child, value)
        }
        scalar => l.insert(i, scalar_of(scalar)).map_err(|e| e.to_string()),
    }
}

fn fill_map(m: &LoroMap, value: &Value) -> Result<(), String> {
    if let Value::Object(fields) = value {
        for (k, v) in fields {
            write_map(m, k, v)?;
        }
    }
    Ok(())
}

fn fill_list(l: &LoroList, value: &Value) -> Result<(), String> {
    if let Value::Array(items) = value {
        for (i, v) in items.iter().enumerate() {
            write_list(l, i, v)?;
        }
    }
    Ok(())
}

/// A Loro value as JSON (map keys sorted). Binary shows as its length.
#[must_use]
pub fn to_json(v: &LoroValue) -> Value {
    match v {
        LoroValue::Null => Value::Null,
        LoroValue::Bool(b) => json!(b),
        LoroValue::Double(d) => serde_json::Number::from_f64(*d).map_or(Value::Null, Value::Number),
        LoroValue::I64(i) => json!(i),
        LoroValue::Binary(b) => json!({ "binary_bytes": b.len() }),
        LoroValue::String(s) => json!(s.as_str()),
        LoroValue::List(items) => Value::Array(items.iter().map(to_json).collect()),
        LoroValue::Map(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), to_json(&fields[k]));
            }
            Value::Object(out)
        }
        LoroValue::Container(id) => json!({ "container": id.to_string() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> LoroDoc {
        let doc = LoroDoc::new();
        let song = doc.get_map("song");
        song.insert("tempo", 78).unwrap();
        song.insert("key", "G").unwrap();
        let sections = song.insert_container("sections", LoroList::new()).unwrap();
        sections.insert(0, "verse").unwrap();
        sections.insert(1, "chorus").unwrap();
        doc.commit();
        doc
    }

    fn ops(v: &Value) -> Vec<Op> {
        parse_ops(v).unwrap()
    }

    fn apply_all(doc: &LoroDoc, v: &Value) -> Result<(), String> {
        for op in ops(v) {
            run(doc, &op)?;
        }
        doc.commit();
        Ok(())
    }

    fn value(doc: &LoroDoc) -> Value {
        to_json(&doc.get_deep_value())
    }

    #[test]
    fn ops_parse_from_json_and_paths_from_either_form() {
        let parsed = ops(&json!([
            { "op": "set", "path": ["song", "tempo"], "value": 80 },
            { "op": "set", "path": "/song/sections/0", "value": "intro" },
            { "op": "insert", "path": "song/sections", "value": "bridge" },
            { "op": "delete", "path": ["song", "sections", 1] },
            { "op": "splice", "path": ["song", "lyrics"], "index": 0, "insert": "Hi" },
            { "op": "undo", "apply": "abc" },
        ]));
        assert_eq!(
            parsed[1],
            Op::Set {
                path: vec!["song".into(), "sections".into(), "0".into()],
                value: json!("intro"),
            }
        );
        assert_eq!(
            parsed[3],
            Op::Delete {
                path: vec!["song".into(), "sections".into(), "1".into()]
            }
        );
        assert!(parse_ops(&json!([])).is_err(), "nothing to apply");
        assert!(parse_ops(&json!({ "op": "set" })).is_err(), "a list of ops");
        let e = parse_ops(&json!([{ "op": "move" }])).unwrap_err();
        assert!(e.contains("op 0") && e.contains("unknown op"), "{e}");
        assert!(parse_ops(&json!([{ "op": "set", "path": "a//b", "value": 1 }])).is_err());
    }

    #[test]
    fn set_insert_delete_and_splice_edit_the_doc() {
        let d = doc();
        apply_all(
            &d,
            &json!([
                { "op": "set", "path": ["song", "tempo"], "value": 82 },
                { "op": "set", "path": ["song", "capo"], "value": { "fret": 2, "on": true } },
                { "op": "insert", "path": ["song", "sections"], "index": 1, "value": "bridge" },
                { "op": "delete", "path": ["song", "key"] },
                { "op": "set", "path": ["notes", "who"], "value": "drums" },
            ]),
        )
        .unwrap();
        let v = value(&d);
        assert_eq!(v["song"]["tempo"], json!(82));
        assert_eq!(v["song"]["capo"], json!({ "fret": 2, "on": true }));
        assert_eq!(v["song"]["sections"], json!(["verse", "bridge", "chorus"]));
        assert!(v["song"].get("key").is_none());
        assert_eq!(v["notes"]["who"], json!("drums"), "a new root map is made");

        let lyrics = d
            .get_map("song")
            .insert_container("lyrics", crdt::loro::LoroText::new())
            .unwrap();
        lyrics.insert(0, "Amazing grace").unwrap();
        d.commit();
        apply_all(
            &d,
            &json!([{ "op": "splice", "path": ["song", "lyrics"], "index": 8, "delete": 5, "insert": "love" }]),
        )
        .unwrap();
        assert_eq!(value(&d)["song"]["lyrics"], json!("Amazing love"));
        apply_all(
            &d,
            &json!([{ "op": "set", "path": ["song", "lyrics"], "value": "Be thou" }]),
        )
        .unwrap();
        assert_eq!(
            value(&d)["song"]["lyrics"],
            json!("Be thou"),
            "a string replaces a text's content"
        );
    }

    #[test]
    fn the_schema_check_keeps_a_fields_kind_and_the_paths_real() {
        let d = doc();
        let refuse = |v: Value| {
            let op = ops(&json!([v])).remove(0);
            run(&d.fork(), &op).unwrap_err()
        };
        let e = refuse(json!({ "op": "set", "path": ["song", "tempo"], "value": "fast" }));
        assert!(e.contains("schema") && e.contains("number"), "{e}");
        let e = refuse(json!({ "op": "set", "path": ["song", "sections"], "value": "verse" }));
        assert!(e.contains("array"), "{e}");
        let e = refuse(json!({ "op": "insert", "path": ["song", "tempo"], "value": 1 }));
        assert!(e.contains("is a value, not a map, list or text"), "{e}");
        let e = refuse(json!({ "op": "insert", "path": ["song"], "value": 1 }));
        assert!(e.contains("not a list"), "a map is no list: {e}");
        let e = refuse(json!({ "op": "delete", "path": ["song", "sections", 5] }));
        assert!(e.contains("past the end"), "{e}");
        let e = refuse(json!({ "op": "set", "path": ["song", "nope", "x"], "value": 1 }));
        assert!(e.contains("nothing at"), "{e}");
        let e = refuse(json!({ "op": "set", "path": ["song"], "value": {} }));
        assert!(e.contains("root"), "{e}");
        let e = refuse(json!({ "op": "splice", "path": ["song", "sections"], "index": 0 }));
        assert!(e.contains("not a text"), "{e}");
        // null changes a kind, in two steps.
        apply_all(
            &d,
            &json!([
                { "op": "set", "path": ["song", "tempo"], "value": null },
                { "op": "set", "path": ["song", "tempo"], "value": "rubato" },
            ]),
        )
        .unwrap();
        assert_eq!(value(&d)["song"]["tempo"], json!("rubato"));
    }

    #[test]
    fn a_version_is_the_docs_frontiers() {
        let empty = LoroDoc::new();
        assert_eq!(version(&empty), "empty");
        let d = doc();
        let v1 = version(&d);
        assert!(v1.contains('@'), "{v1}");
        apply_all(
            &d,
            &json!([{ "op": "set", "path": ["song", "tempo"], "value": 90 }]),
        )
        .unwrap();
        assert_ne!(version(&d), v1, "an edit moves the version");
        assert_eq!(
            version(&d.fork()),
            version(&d),
            "the same doc, the same version"
        );
    }

    #[test]
    fn an_undo_manager_reverts_one_apply_over_a_peers_later_edit() {
        // What `undo` relies on: one apply is one step, and undoing it
        // leaves a peer's later edit alone.
        let served = doc();
        let mut undo = UndoManager::new(&served);
        undo.set_merge_interval(0);
        apply_all(
            &served,
            &json!([{ "op": "set", "path": ["song", "tempo"], "value": 90 }]),
        )
        .unwrap();
        undo.record_new_checkpoint().unwrap();
        // A peer edits another field, and the server imports it.
        let peer = served.fork();
        peer.get_map("song").insert("key", "A").unwrap();
        peer.commit();
        served
            .import(
                &peer
                    .export(crdt::loro::ExportMode::updates(&served.oplog_vv()))
                    .unwrap(),
            )
            .unwrap();
        assert!(undo.undo().unwrap());
        served.commit();
        let v = value(&served);
        assert_eq!(v["song"]["tempo"], json!(78), "the apply is reverted");
        assert_eq!(v["song"]["key"], json!("A"), "the peer's edit stays");
    }

    #[test]
    fn a_commit_message_carries_who() {
        let by = Attribution {
            created_by: Some("bot:1".into()),
            created_by_name: Some("Atlas".into()),
            on_behalf_of: Some("user:2".into()),
        };
        let msg: Value = serde_json::from_str(&commit_message("a1", &by)).unwrap();
        assert_eq!(msg["created_by"], json!("bot:1"));
        assert_eq!(msg["on_behalf_of"], json!("user:2"));
        assert_eq!(msg["apply"], json!("a1"));
    }
}
