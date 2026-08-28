//! The org lane's permit tables — one per mounted vox service — plus the
//! coverage / dry-run reporting that makes the gate's blind spots visible.
//!
//! # Why this module exists
//!
//! [`crate::org_layer_router`] mounts ~70 services; until now exactly two
//! (`VaultSyncRpc`, `MediaService`) carried a [`ServicePermits`] table, and
//! the other ~68 fell through the gate's [`UnlistedPolicy::Allow`]
//! *silently*. Nothing at boot said "66 of your services are unchecked", so
//! the gap sat there. This module (a) supplies a table for every mounted
//! service, (b) derives the mounted-service list ONCE so the permit tables,
//! the schema stamps, and the router can't drift apart, and (c) reports
//! coverage + a static dry-run at boot.
//!
//! [`UnlistedPolicy::Allow`]: architect::permissions_gate::UnlistedPolicy
//!
//! # The model (see `libs/architect/permissions`)
//!
//! A [`MethodPermit`] says "this method needs `<action>` on `<resource>`".
//! The gate checks the *coarse* resource — `vault/{path}` widens to
//! `vault/**` — because the method-level gate runs before argument decode;
//! argument-exact checks are the service impl's job (same engine, finer
//! resource). Methods of a tabled service that are NOT listed are DENIED
//! (fail-closed), which is why [`coverage`] reports them by name.
//!
//! ## Actions used here
//!
//! Only `read` / `write` / `comment` / `download` — the four the built-in
//! `member` role grants. **`admin` is deliberately absent.** The org lane
//! runs `RoleEngine::with_default_user_role("member")` and nothing calls
//! `set_member`, so today *every* validated user is a member and *nobody*
//! is an owner: a single `admin` permit would lock every human out of that
//! method the moment enforcement is switched on. Admin-tier permits wait
//! for per-row membership sync.
//!
//! `.audited()` marks the methods worth an audit line even when allowed:
//! deletes, money movements, outbound sends, downloads.
//!
//! ## Resource namespaces
//!
//! One per domain, path-shaped, so role rules can later be written per
//! domain (`Rule::new("finance/**", &["read"])`):
//!
//! | prefix | lane |
//! |---|---|
//! | `vault/**` | vault files + the read-only vault graph |
//! | `media/**`, `attachments/**` | blobs |
//! | `doc/**` | per-file CRDT sync + presence |
//! | `public/**` | reachable WITHOUT a session (see below) |
//! | everything else | one prefix per feature slice |
//!
//! Resources are written as plain `domain/**` rather than `domain/{arg}`
//! templates: the gate widens `{arg}` to `**` anyway, and inventing an
//! argument name that does not match the trait would bake in a wrong
//! interpolation the day arg-level checks land. `vault/{path}` and
//! `media/{content_hash}` keep their templates because those names were
//! verified against their traits.
//!
//! ## `public/**` — the un-authenticated hole, made explicit
//!
//! `RoleEngine` denies [`Principal::Anonymous`] everything, so a table on
//! `AuthService` would make sign-in impossible the moment enforcement is
//! on — the caller signing IN has no identity yet. Rather than leaving
//! those services untabled (invisible again), their permits point at
//! `public/…` and the gate's engine gains a [`ScopeEngine`] granting
//! `public/**` to everyone. Two services are public:
//!
//! - **`AuthService`** — every method takes the session token as an
//!   ARGUMENT and validates it itself (`org_members_for_token`,
//!   `current_session`); the gate has nothing to add and would only break
//!   the sign-in path.
//! - **`PermissionsService`** — the capability oracle answers *about the
//!   caller*, from the same engine + resolver the gate enforces with. An
//!   anonymous caller gets an empty manifest, which is the correct answer,
//!   not a leak.
//!
//! [`ScopeEngine`]: architect_permissions::ScopeEngine

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use architect_permissions::{
    Action, AuditEvent, AuditSink, BoxIdentityFuture, IdentityResolver, MethodPermit,
    PermissionEngine, Principal, ServicePermits,
};
use vox::ServiceDescriptor;

// ── Table-building helpers ───────────────────────────────────────────────

const fn rd(m: &'static str, r: &'static str) -> MethodPermit {
    MethodPermit::new(m, Action::READ, r)
}
const fn wr(m: &'static str, r: &'static str) -> MethodPermit {
    MethodPermit::new(m, Action::WRITE, r)
}
/// Write + audit line even on allow — deletes, money, outbound effects.
const fn wa(m: &'static str, r: &'static str) -> MethodPermit {
    MethodPermit::new(m, Action::WRITE, r).audited()
}
/// Comment-tier write: allowed to `member` AND grantable to share guests.
const fn cm(m: &'static str, r: &'static str) -> MethodPermit {
    MethodPermit::new(m, Action::COMMENT, r)
}
/// Bulk content leaving the server.
const fn dl(m: &'static str, r: &'static str) -> MethodPermit {
    MethodPermit::new(m, Action::DOWNLOAD, r).audited()
}

/// `table!(CONST, "service", "resource/**", [rd "list", wa "delete"])`
macro_rules! table {
    ($name:ident, $service:literal, $res:expr, [$($f:ident $m:literal),* $(,)?]) => {
        const $name: ServicePermits = ServicePermits {
            service: $service,
            methods: &[$($f($m, $res)),*],
        };
    };
}

/// The resource glob every principal — including [`Principal::Anonymous`] —
/// holds, via the gate's public [`architect_permissions::ScopeEngine`].
pub const PUBLIC_GLOB: &str = "public/**";

// ── Platform lane ────────────────────────────────────────────────────────

/// Self-guarding: every method validates the token it is handed. See the
/// module docs on `public/**`.
///
/// **`sign_up_email_password` is deliberately NOT public.** architect-auth
/// has no email/password signup toggle — `email_password_enabled` gates
/// sign-IN too, and `disable_signup` / `signup_enabled` are OneTap- and
/// SIWE-specific — so open self-registration was on, reachable by anyone,
/// and the org lane hands every validated user the `member` role
/// (`DEFAULT_ORG_ROLE`). That made enforcement bypassable in a single
/// call: sign up, become a member, read the org. Verified reachable on
/// production 2026-08-08 (the only error was password length).
///
/// Pointing it at `auth/signup` instead means the gate refuses anonymous
/// callers while leaving sign-in, whoami and session refresh public — so
/// existing members can still provision accounts, and nobody who isn't
/// one can. Removing it from the table entirely would fail closed for
/// everyone (tabled-but-unlisted is a deny), which would also block the
/// operator.
const AUTH: ServicePermits = ServicePermits {
    service: "auth",
    methods: &[
        MethodPermit::new("sign_up_email_password", Action::WRITE, "auth/signup").audited(),
        MethodPermit::new("sign_in_email_password", Action::READ, "public/auth"),
        MethodPermit::new("current_session", Action::READ, "public/auth"),
        MethodPermit::new("refresh_session", Action::READ, "public/auth"),
        MethodPermit::new("whoami", Action::READ, "public/auth"),
        MethodPermit::new("sign_out", Action::READ, "public/auth"),
        MethodPermit::new("list_org_members", Action::READ, "public/auth"),
        // Changing someone's login identifier is an operator action, so
        // it sits outside `public/**` for the same reason signup does —
        // an anonymous caller must never reach it. The impl also requires
        // a session that validates against THIS org and records it as
        // `changed_by`, so the gate and the flow agree.
        MethodPermit::new("migrate_user_email", Action::WRITE, "auth/migrate").audited(),
        // Reading the trail exposes former addresses of real people;
        // members only, and audited so the read itself is on the record.
        MethodPermit::new("list_email_history", Action::READ, "auth/migrate").audited(),
        // Self-service, but NOT public: you need a session to change your
        // own password, so gating costs nothing and keeps the anonymous
        // surface as small as the sign-in path requires. Audited — a
        // credential change is worth a line even when allowed.
        MethodPermit::new("change_password", Action::WRITE, "auth/self").audited(),
        // Same tier as the password change: it alters your login
        // identifier, needs a session regardless, and is worth an audit
        // line even on allow.
        MethodPermit::new("change_email", Action::WRITE, "auth/self").audited(),
        // Display name / avatar. Same self-service tier — the session
        // names the account, so there is no target to widen. Not
        // audited: unlike a credential or login identifier, changing
        // your display name is not a security event, and the identity
        // fan-out writes it once per linked org.
        MethodPermit::new("update_profile", Action::WRITE, "auth/self"),
    ],
};

// The capability oracle — answers about the caller only.
table!(PERMISSIONS, "permissions", "public/permissions", [rd "can", rd "capabilities"]);

table!(ATTACHMENTS, "attachments", "attachments/**", [
    wr "initiate_upload", wr "complete_upload", dl "get_download_url",
]);

/// Unchanged from the original hand-written table (verified arg names).
const MEDIA: ServicePermits = ServicePermits {
    service: "media",
    methods: &[
        MethodPermit::new("stat", Action::READ, "media/{content_hash}"),
        MethodPermit::new("read", Action::READ, "media/{content_hash}").audited(),
        // Minting a signed URL for the filesystem media route. THIS call
        // is the authorization point for `/org/{slug}/media/…` — the HTTP
        // route only verifies the signature — so it is audited even on
        // allow: a grant is bulk content leaving the server, and knowing
        // who minted one is the whole audit trail for that path.
        MethodPermit::new("media_grant", Action::DOWNLOAD, "media/**").audited(),
    ],
};

/// Unchanged from the original hand-written table (verified arg names).
const VAULT: ServicePermits = ServicePermits {
    service: "vault-sync",
    methods: &[
        MethodPermit::new("manifest", Action::READ, "vault/**"),
        MethodPermit::new("get_file", Action::READ, "vault/{path}"),
        MethodPermit::new("put_file", Action::WRITE, "vault/{path}"),
        MethodPermit::new("delete_file", Action::WRITE, "vault/{path}"),
        MethodPermit::new("folder_index", Action::READ, "vault/**"),
        MethodPermit::new("set_folder", Action::WRITE, "vault/{path}"),
        MethodPermit::new("open_collab", Action::READ, "vault/{path}"),
        MethodPermit::new("base_views", Action::READ, "vault/{path}"),
    ],
};

table!(VAULT_GRAPH, "vault-graph", "vault/**", [
    rd "backlinks", rd "links", rd "orphans", rd "unresolved", rd "deadends", rd "tags",
]);

table!(SHARE, "share", "shares/**", [
    // Minting and revoking are outward-facing — audited even on allow,
    // like every capability-granting write. Retroactive edits (issue
    // #271 AC 5) change what an existing link can do, so they audit too.
    wa "create_link", wa "update_link", rd "list_links", rd "links_for_target",
    wa "set_link_disabled", wa "delete_link",
    // The per-link access log (views + download receipts) is a read;
    // the org kill switch is the org's biggest sharing decision.
    rd "access_log", wa "set_sharing_disabled", rd "sharing_disabled",
    // The file-request inbox (issue #272): listing the queue is a
    // read; promotion writes into the versioned tree — audited.
    rd "list_incoming", wa "promote_incoming",
]);

// Per-file CRDT: `sync` mutates the doc; presence is ephemeral and never
// persisted (see `crate::presence`).
table!(DOC_SYNC, "doc-sync", "doc/**", [wr "sync"]);
table!(DOC_PRESENCE, "doc-presence", "doc/presence/**", [rd "presence"]);

// ── Agent lane ───────────────────────────────────────────────────────────

#[cfg(feature = "plugin-agent")]
table!(AGENT_TASKS, "agent-tasks", "agent/tasks/**", [
    rd "read_queue", wr "claim_agent_task", wr "set_agent_task_status",
    wr "complete_agent_task", wr "link_agent_task_to_session", rd "list_agent_task_links",
]);

#[cfg(feature = "plugin-agent")]
table!(AGENT_SESSIONS, "agent-sessions", "agent/sessions/**", [
    wr "create_session", rd "read_session", rd "list_sessions", wr "rename_session",
    wr "pin_session", wr "archive_session", wa "delete_session", wr "save_composer_draft",
]);

// `dispatch_turn` runs a model turn (spends tokens, may touch the repo).
#[cfg(feature = "plugin-agent")]
table!(AGENT_TURNS, "agent-turns", "agent/turns/**", [
    wa "dispatch_turn", wr "cancel_turn", wr "resume_session",
]);

#[cfg(feature = "plugin-agent")]
table!(AGENT_THREADS, "agent-threads", "agent/threads/**", [
    rd "list_messages", rd "read_message", cm "append_note",
]);

// The three `subscribe_*(id, tx)` calls collapsed into one unfiltered
// `#[subscribe]` stream — the envelope names its session and subscribers
// filter client-side — so this is now one read over every agent event.
#[cfg(feature = "plugin-agent")]
table!(AGENT_SUBSCRIPTIONS, "agent-subscriptions", "agent/events/**", [
    rd "events",
]);

#[cfg(feature = "plugin-agent")]
table!(AGENT_DISCOVERY, "agent-discovery", "agent/discovery/**", [
    rd "list_models", rd "list_skills", rd "list_capabilities", rd "backend_health",
]);

#[cfg(feature = "plugin-agent")]
table!(AGENT_ROUTINES, "agent-routines", "agent/routines/**", [
    rd "list_routines", wr "create_routine", wr "set_routine_paused",
    wa "run_routine", wa "delete_routine",
]);

// The runner registry. Reads are ordinary — anyone who can see the
// org can see which machines serve it. Registering and deregistering
// are administrative: a runner declares what it may execute, so being
// able to write here is being able to grant yourself the right to run
// code for this org.
//
// `heartbeat_backend` is a write rather than a read because it is the
// liveness signal routing depends on — a caller who could forge it
// could keep a dead machine in the routing pool.
// Run records. Reads are ordinary; writes are the runner reporting
// its own progress, which is a member action, not an admin one — a
// runner that cannot say what it did is useless.
#[cfg(feature = "plugin-agent")]
table!(AGENT_RUNS, "agent-runs", "agent/runs/**", [
    rd "get_run", rd "list_runs",
    wr "start_run", wr "beat_run", wr "finish_run",
    wr "archive_run", wr "sweep_stale_runs",
]);

// The grill queue. Asking is a runner write; answering is the human
// half of a human-in-the-loop decision, so both are member writes
// rather than admin.
// Live run state. Reading is ordinary; publishing is the runner
// narrating its own work.
#[cfg(feature = "plugin-agent")]
table!(AGENT_RUN_STREAM, "agent-run-stream", "agent/runs/**", [
    rd "snapshot", wr "publish",
]);

// The subscribe half is its own vox service with its own descriptor.
#[cfg(feature = "plugin-agent")]
table!(AGENT_RUN_EVENTS, "agent-run-events", "agent/runs/**", [
    rd "run_events",
]);

#[cfg(feature = "plugin-agent")]
table!(AGENT_QUESTIONS, "agent-questions", "agent/questions/**", [
    rd "unresolved_questions", rd "questions_for_ticket",
    rd "list_pending_questions", rd "question_ticket",
    wr "ask_question", wr "answer_question",
]);

#[cfg(feature = "plugin-agent")]
table!(AGENT_BACKENDS, "agent-backends", "agent/runners/**", [
    rd "list_backends", rd "backend_health", rd "backends_by_kind",
    wr "heartbeat_backend", wa "upsert_backend", wa "remove_backend",
]);

// ── Work lane (projects / goals / milestones / workstreams / tasks) ──────

table!(PROJECT, "project", "projects/**", [
    rd "list", rd "get", rd "get_by_path", wr "create", wr "update", wr "rename", wa "delete",
    // Parts. `wr` rather than `wa` for the mutations: naming a division
    // of a project's work is ordinary editing, not administration —
    // `project.part.unit` is explicit that a part costs nothing, and a
    // permission that treated adding one as a privileged act would be
    // pricing it.
    rd "parts", wr "add_part", wr "rename_part", wr "remove_part",
    // Promotion. `pieces` is a listing; promoting and demoting create
    // and remove a page, which is what `create` and `delete` are gated
    // as — so they match those rather than the part verbs.
    rd "pieces", wr "promote_part", wa "demote_project",
    // Deliverables. `client_deliverables` is `rd` like every other
    // listing — what makes it a client view is that it filters, not that
    // it is reached differently.
    rd "divergences", wr "attach_component", wr "detach_component",
    // Merge ends one project's separate existence, which is `delete`'s
    // weight even though nothing is deleted.
    wa "merge",
    rd "setlist", wr "set_setlist",
    // Adoption declares a tree to be a project. A write, not an admin
    // act — it creates one page and moves nothing.
    wr "adopt",
    rd "deliverables", wr "declare_deliverable", wa "withdraw_deliverable",
    rd "deliverable_items", rd "client_deliverables",
]);
table!(PROJECT_STREAM, "project-stream", "projects/**", [rd "events"]);
table!(GOAL, "goal", "goals/**", [
    rd "list", rd "get", rd "get_by_path", wr "create", wr "update", wr "rename", wa "delete",
]);
table!(GOAL_STREAM, "goal-stream", "goals/**", [rd "events"]);
table!(MILESTONE, "milestone", "milestones/**", [
    rd "list", rd "get", rd "get_by_path", wr "create", wr "update", wr "rename", wa "delete",
]);
table!(MILESTONE_STREAM, "milestone-stream", "milestones/**", [rd "events"]);
table!(WORKSTREAM, "workstream", "workstreams/**", [
    rd "list", rd "get", rd "get_by_path", wr "create", wr "update",
    wr "set_status", wa "delete", rd "rollup",
]);
table!(WORKSTREAM_STREAM, "workstream-stream", "workstreams/**", [rd "events"]);
table!(FILES, "files", "files/**", [
    wr "create_root", rd "list_roots", rd "get_root", rd "browse", rd "drive_browse", rd "tree_browse",
    rd "chain", wr "checkpoint_now",
    // Cadence engine (issue #260): activity hints and the per-root
    // Ignore set. A hint can cause a capture, so it is a write.
    wr "hint_activity", rd "snapshots", rd "ignore_set", wr "set_ignore_set",
    // Curation (issue #261). Naming and starting an iteration are
    // ordinary writes; dropping a name and running GC carry an audit
    // line even on allow (`wa`, like every `delete` above) because both
    // can end an object's protection. Still member tier — this lane has
    // no admin permits at all (see the module doc).
    wr "name_version", rd "list_named_versions", rd "resolve_named_version",
    wa "unname_version", wr "start_project_version", rd "list_project_versions",
    wa "gc_root",
    // Hydration (issue #263). Dehydrate carries an audit line even on
    // allow (`wa`): it replaces live-tree content with a stub, and
    // although the content survives in the store, it is the one write
    // here that makes files non-resident. Hydrate restores content —
    // an ordinary write. Applying policy does both in bulk.
    wa "dehydrate", wr "hydrate", rd "hydration_policy", wr "set_hydration_policy",
    wa "apply_hydration_policy",
    // Project Version restart (issue #268). Restarting reshapes the
    // whole live tree and copy-forward can overwrite versioned files —
    // audited writes; time-travel browsing is an ordinary read.
    wa "restart_project_version", rd "browse_at", wa "copy_forward",
    // Divergent versions (issue #264): listing is a read; settling
    // writes a merge checkpoint and rewrites live-tree files (audited).
    rd "divergences", wa "resolve_divergence",
    // Derived media (issue #269). Requesting a rendition may generate
    // it (an expensive transcode) and cache it, but it never mutates
    // the versioned tree — a read from the caller's point of view.
    // `rendition_at` is the same call pinned to a past version (the
    // Review page's switcher, issue #270).
    rd "rendition", rd "rendition_at",
    // Reviews (issue #270). The Review page's audience includes
    // share-link guests, so the feedback verbs sit at comment tier
    // (`cm`, like `add_comment` / `post_message`): get-or-create runs
    // when feedback starts, and posting writes a comment page. Pure
    // lookups are reads. Deleting a comment removes someone's
    // feedback, so it carries an audit line even on allow, like the
    // other `delete` verbs.
    rd "find_review", cm "review_for_file", rd "list_reviews", rd "review_comments",
    cm "add_review_comment", wa "delete_review_comment",
]);
table!(FILES_STREAM, "files-stream", "files/**", [rd "events"]);

// ── Files v2 lanes ───────────────────────────────────────────────────────
//
// One table per lane, mirroring `files_proto::service`'s split and the
// spec sections each lane owns (`features/files/spec/files.md`). The v1
// `FILES` table above stays until the last caller moves off it.
//
// Tiering is decided per method from what the method DOES, not from its
// name. The rules applied here:
//   `rd` — reads nothing but structure or metadata out.
//   `wr` — changes state, recoverable from history.
//   `wa` — audited: destroys, displaces, grants, or reaches outside.
//   `dl` — bulk content leaving the server.

// Root lifecycle and adoption (`files.adopt.*`).
//
// `adopt` is a write rather than an audited one: it moves, copies and
// renames nothing (`files.adopt.in-place`), so the worst it does is
// start reading a tree. `release` IS audited — it removes the org's
// record of a root, and although every byte survives on disk, the org
// stops being able to find them.
table!(FILES_ROOTS, "files-roots", "files/**", [
    wr "adopt", wr "resume_adoption", wr "pause_adoption", rd "adoption_progress",
    rd "list", rd "get", wr "rename_root", wa "release",
    // `wa`, matching `release`: it changes which roots this org's
    // servers hold, and although it moves no byte and destroys nothing,
    // "who hosts what" is the same class of fact as "who can find
    // what".
    wa "host_structure",
]);

// The namespace and the replicated catalogue (`files.catalogue.*`).
// Structure out, nothing in — every method is a read.
table!(FILES_TREE, "files-tree", "files/**", [
    rd "browse", rd "resolve", rd "entry", rd "catalogue", rd "changes_since",
    rd "freshness",
]);

// The write surface (`files.write.surface`) — the lane that did not
// exist at all until now.
//
// `delete_paths` is audited even though deletion is a checkpoint and the
// content survives in history: the live tree is what every other
// application on that NAS sees, and making a file vanish from it is the
// act a human needs to be able to trace. `archive` is `dl` — a selection
// can be a whole root, and that is bulk content leaving the server.
table!(FILES_WRITE, "files-write", "files/**", [
    wr "create_dirs", wr "rename", wr "move_paths", wr "copy_paths",
    wa "delete_paths", dl "archive",
]);

// Getting content in (`files.write.upload`).
//
// `complete` is audited because it can displace an existing file:
// `OnConflict::Replace` checkpoints the outgoing content first, so
// nothing is lost, but a file at a path stopped being the file that was
// there. The rest of the lane is planning and accounting.
// `send_bytes` is the ingress byte lane. A write, not `dl` — that tier
// is for content LEAVING the server — and not audited: it moves bytes
// into a staging file outside the tree and displaces nothing. `complete`
// is where a file can stop being the file that was there, and that is
// the audited one.
table!(FILES_UPLOAD, "files-upload", "files/**", [
    wr "begin", rd "progress", wa "complete", wr "abort", rd "pending",
    wr "send_bytes",
]);

// History, divergence and restore (`files.version.*`,
// `files.concurrency.*`).
//
// `hold` is a write and not a read: it publishes a signal other clients
// act on. It is emphatically NOT audited — an advisory signal that
// generated an audit line every heartbeat would drown the log it shares
// with the acts that matter. `restore` and `resolve_divergence` are
// audited because both rewrite live-tree files.
table!(FILES_VERSION, "files-version", "files/**", [
    rd "chain", wr "checkpoint", rd "snapshots", wr "hold", rd "occupancy",
    rd "divergences", wa "resolve_divergence", wa "restore", wr "keep_snapshot",
]);

// Named and Project Versions.
//
// `unname_version` is audited because a name is what exempts a version
// from retention collection — dropping one can end an object's
// protection. `restart_project_version` reshapes the whole live tree.
table!(FILES_CURATION, "files-curation", "files/**", [
    wr "name_version", wa "unname_version", rd "named_versions", rd "resolve_name",
    wr "start_project_version", rd "project_versions", wa "restart_project_version",
]);

// Facets, ignoring, hydration and devices (`files.facet.*`,
// `files.ignore.*`, `files.sync.selective`, `files.device.control`).
//
// `hydrate` carries a `resident` flag, so the same method both fetches
// and releases; the releasing half replaces live-tree content with a
// stub, which is the one write here that makes files non-resident —
// audited for the same reason v1's `dehydrate` was. `revoke_device` cuts
// a device off AND destroys its local copy of org content.
//
// `enroll_device` is audited too, and is the widest of them: it admits a
// machine to this org's whole commit graph. It is a member's call
// because a member sitting at the machine is the authority pairing has —
// but "which laptops hold this org" is exactly what an operator reads an
// audit log to find out. `coordinator` is a plain read: the org's own
// endpoint id is the address it publishes, not a secret.
table!(FILES_SYNC, "files-sync", "files/**", [
    rd "facets", wr "map_facet", rd "ignore_set", wr "set_project_ignores",
    rd "subscription", wr "subscribe", wr "pin", wa "hydrate",
    rd "devices", wr "set_transfer_policy", wa "revoke_device",
    wa "enroll_device", rd "coordinator",
]);

// Replica sync — the commit graph and the chunks under it
// (`files.peering.replication`).
//
// A peer hosting the same org pulls structure over this: `heads` and
// `object` walk the graph, `manifest` says what a file is made of, and
// `chunks` moves the bytes. Every method is a read, and `chunks` is
// `dl` for the same reason `archive` is — it is how a whole library
// leaves, a batch at a time, and "who pulled a copy of everything"
// is the question an audit log exists to answer.
//
// This lane was implemented, tested, and mounted by nothing: peer
// replication could not happen on a real server, only in a harness
// that mounted it itself. Which is why the integration suite now
// serves `org_router_guarded` rather than a router of its own.
/// The replica lane's own coarse resource — defined with the peer model
/// it belongs to, since a device gates on the same string.
///
/// Members are unaffected: their rule is `**`.
pub use files::peer::REPLICA_RESOURCE;

/// The replica lane's rows, defined once with the peer model.
///
/// **Not a `table!` of its own.** A device installs the same rows as the
/// only table its gate has, and two copies would be two things to keep in
/// step — with the failure mode being a method a server serves and a
/// device refuses, or the reverse. `chunk_ranges` is `download` there for
/// the same reason `chunks` is: it is how a whole library leaves, a window
/// at a time.
const FILES_REPLICA: ServicePermits = files::peer::REPLICA_PERMITS;

// Who may see and change what (`files.access.*`).
//
// Every mutation here is audited without exception: granting, revoking
// and minting a link all change who can reach an org's content, and
// `files.access.granularity` makes the blast radius of a wrong grant a
// whole subtree. `effective` is a read of the caller's own permissions.
table!(FILES_ACCESS, "files-access", "files/**", [
    wa "grant", wa "revoke", rd "grants", rd "effective",
    wa "create_share", wa "set_share_disabled", rd "shares",
]);

// Hand-organisation and accountability (`files.organise.*`).
//
// Tagging is an ordinary write: `files.organise.manual` says a tag
// produces a view and never folder membership, so it moves nothing and
// changes no path. `activity` is a read of the feed.
// The byte lane's stream sibling. `dl` — this IS bulk content leaving
// the server, and it is the only method on the whole v2 surface that
// moves file bytes.
table!(FILES_MEDIA_STREAM, "files-media-stream", "files/**", [dl "bytes"]);

// The byte lane's RPC half: tickets and derived previews. Nothing here
// returns content inline — a read mints a ticket redeemed on the stream
// lane above — so these are `rd` rather than `dl`. `handoff` is `wr`: it
// hands a path to an editor, which is the point at which something else
// starts writing.
table!(FILES_MEDIA, "files-media", "files/**", [
    rd "read", rd "read_at", rd "read_content",
    rd "renditions", rd "rendition", wr "handoff",
]);

// `files.index.*`. `extract` is `wr` — it derives and stores a sidecar,
// which `files.index.portable` puts beside the source — and the rest
// read what extraction produced.
table!(FILES_SEARCH, "files-search", "files/**", [
    rd "search", rd "extract_state", rd "pending", wr "extract",
]);

// The guest lane (`files.review.*`). Comments are `cm`, the tier that
// exists for exactly this: a reviewer may say things about content
// without being able to change it. `delete_comment` is `wa` — removing
// somebody else's words is the one act here that destroys.
table!(FILES_REVIEW, "files-review", "files/**", [
    rd "scope", rd "review", rd "playback", rd "comments",
    cm "comment", wa "delete_comment", rd "for_file",
]);

// Content across a server boundary. `offer` and `withdraw` are audited
// without exception, for the same reason every mutation in the access
// lane is: they change who can reach an org's content, and here the
// "who" is on someone else's server. `browse_offered` answers a remote
// receiver and authenticates a secret rather than a session, so it is a
// read that carries an audit line — the one call on this surface whose
// caller is not a member of this org.
//
// `read_offered` and `fetch_offered` are the same case with bytes
// attached: `fetch_offered` is the second method on the v2 surface that
// moves file content, and it does so to a caller on another server, so
// `dl` here is the tier it would have earned even without the audit.
//
// Which is why the three `*_offered` methods sit on `public/**` rather
// than `files/**`, and it is not a relaxation: their credential is the
// offer secret, validated at one chokepoint (`FilesBackend::live_offer`)
// so a withdrawal binds on the next call of any kind. A receiver on
// another server has no session with this org and never will — asking
// the coarse gate "is this caller a member" can only ever answer no. On
// `files/**` they were mounted, implemented, audited and unreachable
// the moment enforcement came on, which is the same failure as not
// mounting them.
//
// They keep `.audited()`. Self-guarding decides *whether* the call is
// allowed; the audit line is about a copy of someone's content leaving
// the building, and that is worth recording either way.
const FILES_FEDERATION: ServicePermits = ServicePermits {
    service: "files-federation",
    methods: &[
        MethodPermit::new("offer", Action::WRITE, "files/**").audited(),
        MethodPermit::new("withdraw", Action::WRITE, "files/**").audited(),
        MethodPermit::new("offered", Action::READ, "files/**"),
        MethodPermit::new("accept", Action::WRITE, "files/**").audited(),
        MethodPermit::new("remotes", Action::READ, "files/**"),
        MethodPermit::new("forget", Action::WRITE, "files/**").audited(),
        MethodPermit::new("browse_offered", Action::READ, "public/files-offer").audited(),
        MethodPermit::new("read_offered", Action::READ, "public/files-offer").audited(),
        MethodPermit::new("fetch_offered", Action::READ, "public/files-offer").audited(),
    ],
};

table!(FILES_ORGANISE, "files-organise", "files/**", [
    rd "marks", wr "set_tags", wr "set_favourite", rd "tagged", rd "all_tags",
    rd "activity",
]);

// The Files placement layer's ORG lane (issue #262). The operator and
// agent lanes are not here on purpose: they live on the server router,
// which this gate does not cover, because the Storage Location registry
// is deployment-scoped and admitting an org onto a location is an
// operator act rather than a member one. What a member may do is place
// their own org's roots inside grants the operator already issued —
// every method below is refused by the backend itself unless a grant
// covers it.
table!(FILES_STORAGE, "storage", "files/**", [
    rd "list_locations", rd "list_grants", rd "placement", rd "list_placements", rd "usage",
    wr "place_root", wr "add_blob_replica", wr "refresh_usage",
]);
table!(FILES_STORAGE_STREAM, "storage-stream", "files/**", [rd "events"]);
table!(TASK, "task", "tasks/**", [
    rd "list", rd "get", rd "get_by_path", wr "create", wr "update", wr "try_claim",
    rd "reverse_relations", rd "reverse_relations_batch", rd "query", wr "rename", wa "delete",
]);
table!(TASK_STREAM, "task-stream", "tasks/**", [rd "events"]);

table!(TIMER, "timer", "timer/**", [
    wr "start_timer", wr "stop_timer", rd "active_timer", wr "switch_timer", wr "log_session",
    rd "resolve_rate", rd "list_sessions", wr "update_session", wa "delete_session",
    wa "set_org_member_rate", wa "set_project_member_rate",
    rd "list_org_member_rates", rd "list_project_member_rates",
    rd "list_tags", wr "create_tag", wa "delete_tag", wr "attach_tags", wr "detach_tags",
]);
table!(TIMER_STREAM, "timer-stream", "timer/**", [rd "events"]);

table!(THREADS, "threads", "threads/**", [
    rd "list_threads", rd "get_thread", cm "create_thread", rd "list_messages",
    cm "post_message", wr "set_resolved", wa "delete_thread", wa "delete_message",
]);

table!(PREFS, "prefs", "prefs/**", [rd "get", wr "set"]);

// ── Scheduling lane ──────────────────────────────────────────────────────

#[cfg(feature = "plugin-scheduling")]
table!(DAY_TEMPLATES, "day-templates", "scheduling/day-templates/**", [
    rd "list_day_templates", rd "get_day_template", wr "upsert_day_template",
    wa "delete_day_template",
]);
#[cfg(feature = "plugin-scheduling")]
table!(DAY_PLANS, "day-plans", "scheduling/day-plans/**", [
    rd "get_day_plan", wr "upsert_day_plan", wa "delete_day_plan",
]);
#[cfg(feature = "plugin-scheduling")]
table!(CALENDAR_EVENTS, "calendar-events", "scheduling/events/**", [
    rd "list_events", wr "upsert_event", wa "delete_event",
]);
#[cfg(feature = "plugin-scheduling")]
table!(EVENT_TYPES, "event-types", "scheduling/event-types/**", [
    rd "list_event_types", rd "get_event_type", wr "upsert_event_type", wa "delete_event_type",
]);
#[cfg(feature = "plugin-scheduling")]
table!(SCHEDULES, "schedules", "scheduling/schedules/**", [
    rd "list_schedules", rd "get_schedule", wr "upsert_schedule", wa "delete_schedule",
]);
#[cfg(feature = "plugin-scheduling")]
table!(SLOTS, "slots", "scheduling/slots/**", [rd "list_open_slots"]);
#[cfg(feature = "plugin-scheduling")]
table!(BOOKINGS, "bookings", "scheduling/bookings/**", [
    rd "list_bookings", rd "get_booking", wr "create_booking", wr "update_booking_status",
]);
// The slice's one stream: attaching is a read over the whole
// scheduling resource — the event names the sub-resource and
// subscribers filter client-side.
#[cfg(feature = "plugin-scheduling")]
table!(SCHEDULING_STREAM, "scheduling-events", "scheduling/**", [rd "events"]);

// ── Knowledge lane ───────────────────────────────────────────────────────

// Notifications: clients read + flip read-state + prune; creation is
// server-internal (the notifier), so no client-reachable write mints
// rows.
table!(NOTIFY, "notify", "notifications/**", [
    rd "list", wr "mark_read", wr "mark_all_read", wa "delete",
]);
table!(NOTIFY_STREAM, "notify-stream", "notifications/**", [rd "events"]);
table!(INBOX, "inbox", "inbox/**", [
    rd "list_inbox", rd "review_queue", rd "get_inbox_item",
    wr "upsert_inbox_item", wa "delete_inbox_item",
]);
table!(INBOX_STREAM, "inbox-stream", "inbox/**", [rd "events"]);
#[cfg(feature = "plugin-recall")]
table!(RECALL, "recall", "recall/**", [
    rd "list_cards", rd "review_queue", wr "upsert_card", wa "delete_card",
]);
#[cfg(feature = "plugin-recall")]
table!(RECALL_STREAM, "recall-stream", "recall/**", [rd "events"]);
#[cfg(feature = "plugin-contacts")]
table!(CONTACTS, "contacts", "contacts/**", [
    rd "list_contacts", rd "get_contact", wr "upsert_contact", wa "delete_contact",
    rd "list_accounts", wr "upsert_account", wa "delete_account", wa "sync_account",
]);
#[cfg(feature = "plugin-contacts")]
table!(CONTACTS_STREAM, "contacts-stream", "contacts/**", [rd "events"]);
table!(TAGS, "tags", "tags/**", [
    rd "list_tags", rd "get_tag", wr "upsert_tag", wa "delete_tag",
]);
#[cfg(feature = "plugin-scripture")]
table!(SCRIPTURE, "scripture", "scripture/**", [
    rd "translations", rd "chapter", rd "verse", rd "compare", rd "chapter_backlinks",
    rd "lexicon", rd "word_study", rd "occurrences", rd "original_editions", rd "interlinear",
    rd "study", rd "cross_refs", rd "topics_of", rd "verses_for_topic",
]);
table!(LINKS, "links", "links/**", [
    wr "create", wa "delete", rd "get", rd "links_for", rd "graph",
]);
#[cfg(feature = "plugin-fasttrackstudio")]
table!(COLLECTION, "collection", "collections/**", [
    wr "create", rd "get", rd "list", wr "add_item", wr "remove_item", wr "reorder",
]);
table!(RESOURCES, "resources", "resources/**", [rd "transcript"]);

// ── Finance lane (every mutation audited) ────────────────────────────────

#[cfg(feature = "plugin-finance")]
table!(INVOICING, "invoicing", "finance/invoicing/**", [
    wa "generate_invoice", rd "list_invoices", rd "get_invoice", wa "delete_invoice",
    wa "record_invoice_payment", rd "uninvoiced", wa "mark_sent", wa "void_with_credit",
    wa "record_payment", wa "refund_payment", wa "run_schedule_once",
    wa "commit_invoice", wa "void_invoice",
]);
#[cfg(feature = "plugin-finance")]
table!(LEDGER, "ledger", "finance/ledger/**", [
    wa "post_transaction", rd "account_transactions", rd "balances", rd "books", rd "accounts",
]);

// ── Wiki lane ────────────────────────────────────────────────────────────

#[cfg(feature = "plugin-wiki")]
table!(WIKI_SCHEMA, "wiki-schema", "wiki/schema/**", [
    wa "bootstrap", rd "read_schema", rd "read_purpose", wr "write_schema",
    wr "write_purpose", rd "health",
]);
#[cfg(feature = "plugin-wiki")]
table!(WIKI_CATALOG, "wiki-catalog", "wiki/catalog/**", [
    rd "read_index", wa "rebuild_index", wr "append_log",
]);
#[cfg(feature = "plugin-wiki")]
table!(WIKI_RAW, "wiki-raw", "wiki/raw/**", [
    wa "import_raw_source", rd "list_raw_sources", rd "read_raw_source",
    wa "delete_raw_source", wr "rescan_sources", wr "rescan_diff",
]);
#[cfg(feature = "plugin-wiki")]
table!(WIKI_GRAPH, "wiki-graph", "wiki/graph/**", [
    wr "build_graph", rd "relevance", rd "clusters", rd "gaps",
]);
#[cfg(feature = "plugin-wiki")]
table!(WIKI_PAGES, "wiki-pages", "wiki/pages/**", [
    rd "list_pages", rd "read_page", wr "write_page",
]);
#[cfg(feature = "plugin-wiki")]
table!(WIKI_INGEST, "wiki-ingest", "wiki/ingest/**", [
    wr "enqueue_ingest", rd "list_ingest", wr "claim_next_ingest", wr "record_analysis",
    wr "record_pages", wr "fail_ingest", wr "cancel_ingest", wr "retry_ingest",
]);
#[cfg(feature = "plugin-wiki")]
table!(WIKI_LINT, "wiki-lint", "wiki/lint/**", [
    wr "lint", rd "list_findings", wr "resolve_finding",
]);
#[cfg(feature = "plugin-wiki")]
table!(WIKI_SEARCH, "wiki-search", "wiki/search/**", [rd "search"]);
#[cfg(feature = "plugin-wiki")]
table!(WIKI_WATCHER, "wiki-watcher", "wiki/watcher/**", [wr "set_watch", rd "is_watching"]);
#[cfg(feature = "plugin-wiki")]
table!(WIKI_MULTIMODAL, "wiki-multimodal", "wiki/multimodal/**", [wr "extract_images"]);
#[cfg(feature = "plugin-wiki")]
table!(WIKI_REVIEW, "wiki-review", "wiki/review/**", [
    wr "enqueue_review", rd "list_review", wa "apply_review",
]);

// ── Home lane (locations / inventory / meals / fitness) ──────────────────

#[cfg(feature = "plugin-home")]
table!(LOCATIONS, "locations", "locations/**", [
    rd "list", rd "get", wr "create", wr "update", wr "rename", wa "delete",
]);
#[cfg(feature = "plugin-home")]
table!(INVENTORY, "inventory", "inventory/**", [
    rd "list", rd "list_at", rd "get", wr "create", wr "update", wr "rename", wa "delete",
    wr "set_status", wr "set_condition", wr "set_location",
]);
#[cfg(feature = "plugin-mealplan")]
table!(COOKBOOK, "cookbook", "mealplan/cookbook/**", [
    rd "list", rd "get", wr "create", wr "update", wr "rename", wa "delete", wr "import",
    rd "image", wr "put_image",
]);
#[cfg(feature = "plugin-mealplan")]
table!(MEALPLAN, "mealplan", "mealplan/plan/**", [
    rd "list", rd "get", wr "create", wr "update", wr "rename", wa "delete",
    wr "cook", wr "skip", wr "eat_out", rd "can_cook", wr "cook_recipe",
]);
#[cfg(feature = "plugin-mealplan")]
table!(PANTRY, "pantry", "mealplan/pantry/**", [
    rd "list", rd "get", wr "create", wr "update", wr "rename", wa "delete",
    wr "consume", wr "restock", wr "open", rd "find_by_barcode", rd "resolve_barcode",
    wr "add_stock", wr "consume_stock", wr "transfer_stock", wr "inventory_set",
]);
#[cfg(feature = "plugin-mealplan")]
table!(SHOPPING, "shopping", "mealplan/shopping/**", [
    rd "list", rd "get", wr "create", wr "update", wa "delete",
    wr "add_missing_for_recipe", wr "add_recipe_ingredients", wr "add_low_stock",
    wr "add_expired_or_overdue", wr "clear", wr "mark_purchased", wr "mark_have",
    wr "reset", wr "start_from_template", wr "save_as_template",
]);
#[cfg(feature = "plugin-mealplan")]
table!(SUBSTITUTIONS, "substitutions", "mealplan/substitutions/**", [
    rd "list", rd "get", wr "create", wr "update", wa "delete", rd "for_item",
]);
#[cfg(feature = "plugin-fitness")]
table!(BODY, "body", "fitness/body/**", [
    rd "list", rd "get", rd "find_by_kind", wr "create", wr "update", wa "delete", wr "log_entry",
]);
#[cfg(feature = "plugin-fitness")]
table!(EXERCISES, "exercises", "fitness/exercises/**", [
    rd "list", rd "get", rd "find_by_name", wr "create", wr "update", wr "rename", wa "delete",
]);
#[cfg(feature = "plugin-fitness")]
table!(WORKOUTS, "workouts", "fitness/workouts/**", [
    rd "list_routines", rd "get_routine", wr "create_routine", wr "update_routine",
    wa "delete_routine", rd "list_sessions", rd "get_session", wr "create_session",
    wr "update_session", wa "delete_session", wr "log_set", wr "start_from_routine",
]);
#[cfg(feature = "plugin-fitness")]
table!(INTAKE, "intake", "fitness/intake/**", [
    rd "list", rd "get", rd "for_day", wr "create", wr "update", wa "delete",
    wr "log_recipe", wr "log_pantry", wr "log_freeform", wr "log_entry",
]);

// ── Outside-world lane (email / forge) ───────────────────────────────────

#[cfg(feature = "plugin-email")]
table!(EMAIL, "email", "email/**", [
    rd "accounts", rd "list_folders", rd "fetch_envelopes", rd "fetch_message",
    dl "fetch_attachment", wr "set_flags", wr "move_message", wa "delete_message",
    wr "append_draft", wa "send",
]);
#[cfg(feature = "plugin-email")]
table!(EMAIL_PRODUCT, "email-product", "email/outbox/**", [
    rd "list_outbox", wr "submit_draft", wa "approve", wr "cancel",
    rd "derivations", rd "unnotified", wr "mark_notified",
]);
#[cfg(feature = "plugin-forge")]
table!(FORGE_REPOS, "forge-repos", "forge/repos/**", [rd "list_repos", rd "get_repo"]);
#[cfg(feature = "plugin-forge")]
table!(FORGE_ISSUES, "forge-issues", "forge/issues/**", [
    rd "list_issues", rd "get_issue", wr "create_issue", wr "update_issue",
    rd "list_comments", cm "add_comment",
]);
#[cfg(feature = "plugin-forge")]
table!(FORGE_REVIEWS, "forge-reviews", "forge/reviews/**", [
    rd "list_pull_requests", rd "get_pull_request", wr "create_pull_request",
    wr "update_pull_request", rd "list_reviews", wr "request_reviewers",
    wa "merge_pull_request",
]);
#[cfg(feature = "plugin-forge")]
table!(FORGE_CONNECTIONS, "forge-connections", "forge/connections/**", [
    rd "list_connected_repos", rd "repos_for_project",
]);

// ── `#[subscribe]` stream siblings ───────────────────────────────────────
//
// Each stream is a separate vox service with its own descriptor, so it
// needs its own table. Attaching to a change feed is a read over the whole
// resource: the streams are unfiltered — subscribers filter client-side —
// so a subscriber sees every change the org produces for that domain.
table!(VAULT_STREAM, "vault-sync-stream", "vault/**", [rd "changes"]);
#[cfg(feature = "plugin-wiki")]
table!(WIKI_STREAM, "wiki-events", "wiki/**", [rd "changes"]);
#[cfg(feature = "plugin-email")]
table!(EMAIL_STREAM, "email-stream", "email/**", [rd "changes"]);
// Links are org-scoped metadata about mail, not mail — its own
// resource path, so "can read the mailbox" and "can see what mail is
// attached to this project" stay separable.
#[cfg(feature = "plugin-email")]
table!(EMAIL_LINKS, "email-links", "email/links/**", [
    wr "link", wa "unlink", rd "links_for_message", rd "links_for_target",
]);
#[cfg(feature = "plugin-forge")]
table!(FORGE_ISSUES_STREAM, "forge-issues-stream", "forge/issues/**", [rd "issue_events"]);
#[cfg(feature = "plugin-forge")]
table!(FORGE_REVIEWS_STREAM, "forge-reviews-stream", "forge/reviews/**", [rd "review_events"]);

// ── The mount list ───────────────────────────────────────────────────────

/// One service as [`crate::org_layer_router`] mounts it: its descriptor,
/// the permit table the gate installs for it, and the plugin that owns it.
///
/// `permits: None` means "mounted, deliberately ungated" — it would show up
/// in [`coverage`] as untabled. Nothing uses it today (every mounted
/// service has a table); it exists so a future mount that genuinely cannot
/// be tabled is recorded as a decision instead of an omission.
///
/// `plugin` is a `task_plugin::CATALOG` id (`"core"` for platform
/// services); the grouping follows the table in
/// the plugin system. [`mounts_for`] filters on it —
/// that filtered view is what [`crate::org_layer_router`] must match for
/// an org with a deny-list.
pub struct Mount {
    pub descriptor: &'static ServiceDescriptor,
    pub permits: Option<ServicePermits>,
    /// Owning plugin's catalog id (`task_plugin::CATALOG`).
    pub plugin: &'static str,
}

const fn m(
    plugin: &'static str,
    descriptor: &'static ServiceDescriptor,
    permits: ServicePermits,
) -> Mount {
    Mount {
        descriptor,
        permits: Some(permits),
        plugin,
    }
}

/// Every service this build knows how to mount, paired with its permit
/// table and owning plugin — the CATALOG, independent of any org's
/// deny-list. **Keep in lockstep with [`crate::org_layer_router`]** —
/// `permits_cover_router` (in `tests/`) fails the build if the
/// plugin-filtered views diverge. [`crate::schema_stamps`] folds this
/// full list on purpose: a disabled service's stamp is still this
/// build's stamp (skew detection is build-level, not org-level).
#[must_use]
pub fn mounts() -> Vec<Mount> {
    let mut v: Vec<Mount> = vec![
        // Platform
        m(
            "core",
            architect_auth::auth_service_service_descriptor(),
            AUTH,
        ),
        m(
            "core",
            architect_permissions_proto::permissions_service_service_descriptor(),
            PERMISSIONS,
        ),
        m(
            "core",
            attachments_proto::attachment_descriptor(),
            ATTACHMENTS,
        ),
        m("core", media_proto::attachment_media_descriptor(), MEDIA),
        m("core", vault_proto::descriptor(), VAULT),
        m("core", vault_proto::stream_descriptor(), VAULT_STREAM),
        m(
            "core",
            vault_proto::vault_graph_rpc_service_descriptor(),
            VAULT_GRAPH,
        ),
        m(
            "core",
            share_proto::share_service_service_descriptor(),
            SHARE,
        ),
        m("core", crdt::sync::doc_sync_service_descriptor(), DOC_SYNC),
        m(
            "core",
            crdt::sync::doc_presence_service_descriptor(),
            DOC_PRESENCE,
        ),
    ];
    #[cfg(feature = "plugin-agent")]
    v.extend([
        // Agent
        m(
            "agent",
            agent_proto::service::tasks::agent_task_queue_rpc_service_descriptor(),
            AGENT_TASKS,
        ),
        m(
            "agent",
            agent_proto::service::sessions::sessions_rpc_service_descriptor(),
            AGENT_SESSIONS,
        ),
        m(
            "agent",
            agent_proto::service::turn_dispatch::turn_dispatch_rpc_service_descriptor(),
            AGENT_TURNS,
        ),
        m(
            "agent",
            agent_proto::service::threads::threads_rpc_service_descriptor(),
            AGENT_THREADS,
        ),
        m(
            "agent",
            agent_proto::service::subscriptions::subscriptions_stream_service_descriptor(),
            AGENT_SUBSCRIPTIONS,
        ),
        m(
            "agent",
            agent_proto::service::discovery::discovery_rpc_service_descriptor(),
            AGENT_DISCOVERY,
        ),
        m(
            "agent",
            agent_proto::service::routines::routines_rpc_service_descriptor(),
            AGENT_ROUTINES,
        ),
        m(
            "agent",
            agent_proto::service::backends::backends_rpc_service_descriptor(),
            AGENT_BACKENDS,
        ),
        m(
            "agent",
            agent_proto::service::runs::runs_rpc_service_descriptor(),
            AGENT_RUNS,
        ),
        m(
            "agent",
            agent_proto::service::questions::questions_rpc_service_descriptor(),
            AGENT_QUESTIONS,
        ),
        m(
            "agent",
            agent_proto::service::run_stream::run_stream_rpc_service_descriptor(),
            AGENT_RUN_STREAM,
        ),
        m(
            "agent",
            agent_proto::service::run_stream::run_stream_stream_service_descriptor(),
            AGENT_RUN_EVENTS,
        ),
    ]);
    v.extend([
        // Work
        m("core", project::project_service_descriptor(), PROJECT),
        m("core", project::project_stream_descriptor(), PROJECT_STREAM),
        m("core", goal::goal_service_descriptor(), GOAL),
        m("core", goal::goal_stream_descriptor(), GOAL_STREAM),
        m("core", milestone::milestone_service_descriptor(), MILESTONE),
        m(
            "core",
            milestone::milestone_stream_descriptor(),
            MILESTONE_STREAM,
        ),
        m(
            "core",
            workstream::workstream_service_descriptor(),
            WORKSTREAM,
        ),
        m(
            "core",
            workstream::workstream_stream_descriptor(),
            WORKSTREAM_STREAM,
        ),
        m("core", files::files_service_descriptor(), FILES),
        m("core", files::files_stream_descriptor(), FILES_STREAM),
        m("core", files_proto::roots_descriptor(), FILES_ROOTS),
        m("core", files_proto::tree_descriptor(), FILES_TREE),
        m("core", files_proto::write_descriptor(), FILES_WRITE),
        m("core", files_proto::upload_descriptor(), FILES_UPLOAD),
        m("core", files_proto::version_descriptor(), FILES_VERSION),
        m("core", files_proto::curation_descriptor(), FILES_CURATION),
        m("core", files_proto::sync_descriptor(), FILES_SYNC),
        m(
            "core",
            files_sync::sync_service_service_descriptor(),
            FILES_REPLICA,
        ),
        m("core", files_proto::access_descriptor(), FILES_ACCESS),
        m("core", files_proto::organise_descriptor(), FILES_ORGANISE),
        m(
            "core",
            files_proto::federation_descriptor(),
            FILES_FEDERATION,
        ),
        m("core", files_proto::media_descriptor(), FILES_MEDIA),
        m(
            "core",
            files_proto::media_stream_descriptor(),
            FILES_MEDIA_STREAM,
        ),
        m("core", files_proto::search_descriptor(), FILES_SEARCH),
        m("core", files_proto::review_descriptor(), FILES_REVIEW),
        m(
            "core",
            files_storage::storage_service_descriptor(),
            FILES_STORAGE,
        ),
        m(
            "core",
            files_storage::storage_stream_descriptor(),
            FILES_STORAGE_STREAM,
        ),
        m("core", task::task_service_descriptor(), TASK),
        m("core", task::task_stream_descriptor(), TASK_STREAM),
        m(
            "core",
            timer_proto::service::timer_service_rpc_service_descriptor(),
            TIMER,
        ),
        m("core", timer_proto::timer_stream_descriptor(), TIMER_STREAM),
        m(
            "core",
            threads::service::threads_service_rpc_service_descriptor(),
            THREADS,
        ),
        m(
            "core",
            prefs_proto::service::prefs_service_rpc_service_descriptor(),
            PREFS,
        ),
    ]);
    #[cfg(feature = "plugin-scheduling")]
    v.extend([
        // Scheduling
        m(
            "scheduling",
            scheduling_proto::service::day_templates::day_templates_rpc_service_descriptor(),
            DAY_TEMPLATES,
        ),
        m(
            "scheduling",
            scheduling_proto::service::day_plans::day_plans_rpc_service_descriptor(),
            DAY_PLANS,
        ),
        m(
            "scheduling",
            scheduling_proto::service::calendar_events::calendar_events_rpc_service_descriptor(),
            CALENDAR_EVENTS,
        ),
        m(
            "scheduling",
            scheduling_proto::service::event_types::event_types_rpc_service_descriptor(),
            EVENT_TYPES,
        ),
        m(
            "scheduling",
            scheduling_proto::service::schedules::schedules_rpc_service_descriptor(),
            SCHEDULES,
        ),
        m(
            "scheduling",
            scheduling_proto::service::slots::slots_rpc_service_descriptor(),
            SLOTS,
        ),
        m(
            "scheduling",
            scheduling_proto::service::bookings::bookings_rpc_service_descriptor(),
            BOOKINGS,
        ),
        m(
            "scheduling",
            scheduling_proto::scheduling_events_stream_descriptor(),
            SCHEDULING_STREAM,
        ),
    ]);
    v.extend([
        // Knowledge
        m(
            "core",
            inbox_proto::service::inbox::inbox_rpc_service_descriptor(),
            INBOX,
        ),
        m("core", inbox_proto::inbox_stream_descriptor(), INBOX_STREAM),
        m(
            "core",
            notify_proto::notify_rpc_service_descriptor(),
            NOTIFY,
        ),
        m(
            "core",
            notify_proto::notify_stream_descriptor(),
            NOTIFY_STREAM,
        ),
    ]);
    #[cfg(feature = "plugin-recall")]
    v.extend([
        m(
            "recall",
            recall_proto::service::recall::recall_rpc_service_descriptor(),
            RECALL,
        ),
        m(
            "recall",
            recall_proto::recall_stream_descriptor(),
            RECALL_STREAM,
        ),
    ]);
    #[cfg(feature = "plugin-contacts")]
    v.extend([
        m(
            "contacts",
            contacts_proto::service::contacts::contacts_rpc_service_descriptor(),
            CONTACTS,
        ),
        m(
            "contacts",
            contacts_proto::contacts_stream_descriptor(),
            CONTACTS_STREAM,
        ),
    ]);
    v.extend([m(
        "core",
        tag_proto::service::tags::tag_service_rpc_service_descriptor(),
        TAGS,
    )]);
    #[cfg(feature = "plugin-scripture")]
    v.extend([m(
        "scripture",
        scripture::scripture_service_descriptor(),
        SCRIPTURE,
    )]);
    v.extend([m("core", links::links_service_descriptor(), LINKS)]);
    #[cfg(feature = "plugin-fasttrackstudio")]
    v.extend([m(
        "fasttrackstudio",
        collection::collection_service_descriptor(),
        COLLECTION,
    )]);
    v.extend([m(
        "core",
        resources_proto::resources_service_rpc_service_descriptor(),
        RESOURCES,
    )]);
    #[cfg(feature = "plugin-finance")]
    v.extend([
        // Finance
        m(
            "finance",
            finance_proto::service::invoicing::invoicing_rpc_service_descriptor(),
            INVOICING,
        ),
        m(
            "finance",
            finance_proto::service::ledger::ledger_rpc_service_descriptor(),
            LEDGER,
        ),
    ]);
    #[cfg(feature = "plugin-wiki")]
    v.extend([
        // Wiki
        m(
            "wiki",
            wiki_proto::service::schema::schema_rpc_service_descriptor(),
            WIKI_SCHEMA,
        ),
        m(
            "wiki",
            wiki_proto::service::catalog::catalog_rpc_service_descriptor(),
            WIKI_CATALOG,
        ),
        m(
            "wiki",
            wiki_proto::service::raw_layer::raw_layer_rpc_service_descriptor(),
            WIKI_RAW,
        ),
        m(
            "wiki",
            wiki_proto::service::graph::graph_rpc_service_descriptor(),
            WIKI_GRAPH,
        ),
        m(
            "wiki",
            wiki_proto::service::pages::pages_rpc_service_descriptor(),
            WIKI_PAGES,
        ),
        m(
            "wiki",
            wiki_proto::service::ingest::ingest_rpc_service_descriptor(),
            WIKI_INGEST,
        ),
        m(
            "wiki",
            wiki_proto::service::lint::lint_rpc_service_descriptor(),
            WIKI_LINT,
        ),
        m(
            "wiki",
            wiki_proto::service::search::search_rpc_service_descriptor(),
            WIKI_SEARCH,
        ),
        m(
            "wiki",
            wiki_proto::service::events::events_stream_service_descriptor(),
            WIKI_STREAM,
        ),
        m(
            "wiki",
            wiki_proto::service::watcher::watcher_rpc_service_descriptor(),
            WIKI_WATCHER,
        ),
        m(
            "wiki",
            wiki_proto::service::multimodal::multimodal_rpc_service_descriptor(),
            WIKI_MULTIMODAL,
        ),
        m(
            "wiki",
            wiki_proto::service::review::review_rpc_service_descriptor(),
            WIKI_REVIEW,
        ),
    ]);
    #[cfg(feature = "plugin-home")]
    v.extend([
        // Home
        m("home", locations::locations_service_descriptor(), LOCATIONS),
        m("home", inventory::inventory_service_descriptor(), INVENTORY),
    ]);
    #[cfg(feature = "plugin-mealplan")]
    v.extend([
        m(
            "mealplan",
            cookbook::cookbook_service_descriptor(),
            COOKBOOK,
        ),
        m(
            "mealplan",
            mealplan::mealplan_service_descriptor(),
            MEALPLAN,
        ),
        m("mealplan", pantry::pantry_service_descriptor(), PANTRY),
        m(
            "mealplan",
            mealplan::shopping::shopping_service_rpc_service_descriptor(),
            SHOPPING,
        ),
        m(
            "mealplan",
            mealplan::substitutions::substitution_service_rpc_service_descriptor(),
            SUBSTITUTIONS,
        ),
    ]);
    #[cfg(feature = "plugin-fitness")]
    v.extend([
        m("fitness", body::body_service_descriptor(), BODY),
        m(
            "fitness",
            exercises::exercises_service_descriptor(),
            EXERCISES,
        ),
        m("fitness", workouts::workouts_service_descriptor(), WORKOUTS),
        m("fitness", intake::intake_service_descriptor(), INTAKE),
    ]);
    #[cfg(feature = "plugin-email")]
    v.extend([
        // Outside world
        m("email", email_proto::descriptor(), EMAIL),
        m("email", email_proto::product_descriptor(), EMAIL_PRODUCT),
        m("email", email_proto::stream_descriptor(), EMAIL_STREAM),
        m("email", email_proto::links_descriptor(), EMAIL_LINKS),
    ]);
    #[cfg(feature = "plugin-forge")]
    v.extend([
        m(
            "forge",
            git_proto::repo::repo_catalog_rpc_service_descriptor(),
            FORGE_REPOS,
        ),
        m(
            "forge",
            git_proto::issues::issue_tracker_rpc_service_descriptor(),
            FORGE_ISSUES,
        ),
        m(
            "forge",
            git_proto::reviews::review_surface_rpc_service_descriptor(),
            FORGE_REVIEWS,
        ),
        m(
            "forge",
            git_proto::issues::issue_tracker_stream_service_descriptor(),
            FORGE_ISSUES_STREAM,
        ),
        m(
            "forge",
            git_proto::reviews::review_surface_stream_service_descriptor(),
            FORGE_REVIEWS_STREAM,
        ),
        m(
            "forge",
            git_proto::connections::repo_connections_rpc_service_descriptor(),
            FORGE_CONNECTIONS,
        ),
    ]);
    v
}

/// The mounts an org with plugin set `set` actually serves — what
/// [`crate::org_layer_router`] mounts and the permit gate installs
/// tables for. [`mounts`] stays the full catalog; this is the org view.
#[must_use]
pub fn mounts_for(set: &task_plugin::PluginSet) -> Vec<Mount> {
    mounts()
        .into_iter()
        .filter(|m| set.contains(m.plugin))
        .collect()
}

/// Every mounted service's descriptor — the single list
/// [`crate::schema_stamps`] and the permit gate both fold over.
#[must_use]
pub fn mounted_descriptors() -> Vec<&'static ServiceDescriptor> {
    mounts().into_iter().map(|m| m.descriptor).collect()
}

/// Install the permit tables for every service mounted under `set` on
/// `gate` — permits exist only for services that are actually served, so
/// a disabled plugin's tables are not registered (its services aren't
/// dispatchable either; the router refuses them before the gate would).
///
/// Registering a table makes that service's UNLISTED methods fail-closed,
/// which is why [`coverage`] must stay clean — and why it is logged at
/// boot.
#[must_use]
pub fn install_for(
    mut gate: architect::permissions_gate::PermissionsGate,
    set: &task_plugin::PluginSet,
) -> architect::permissions_gate::PermissionsGate {
    for mount in mounts_for(set) {
        if let Some(table) = mount.permits {
            gate = gate.permit(mount.descriptor, table);
        }
    }
    gate
}

/// [`install_for`] with everything enabled — the pre-plugin behaviour.
#[must_use]
pub fn install(
    gate: architect::permissions_gate::PermissionsGate,
) -> architect::permissions_gate::PermissionsGate {
    install_for(gate, &task_plugin::PluginSet::resolve(None))
}

// ── Coverage ─────────────────────────────────────────────────────────────

/// What the permit tables do and do not cover. Computed statically from
/// the descriptors — no I/O, no allocation beyond the report itself.
#[derive(Debug, Default)]
pub struct Coverage {
    /// Services [`mounts`] lists.
    pub services: usize,
    /// …of which carry a permit table.
    pub tabled: usize,
    /// Mounted services with NO table (they fall through `UnlistedPolicy`).
    pub untabled: Vec<&'static str>,
    /// Methods across all mounted services.
    pub methods: usize,
    /// …of which a permit names.
    pub permitted: usize,
    /// `(service, method)` present on the descriptor but missing from its
    /// table — **fail-closed once enforcing**.
    pub uncovered: Vec<(&'static str, &'static str)>,
    /// `(service, method)` named by a table but absent from the descriptor
    /// — a typo or a renamed method; the permit is dead and the real
    /// method (if any) is silently fail-closed.
    pub phantom: Vec<(&'static str, &'static str)>,
}

impl Coverage {
    /// Nothing missing and nothing dead.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.untabled.is_empty() && self.uncovered.is_empty() && self.phantom.is_empty()
    }
}

#[must_use]
pub fn coverage() -> Coverage {
    let mut c = Coverage::default();
    for mount in mounts() {
        c.services += 1;
        c.methods += mount.descriptor.methods.len();
        let name = mount.descriptor.service_name;
        let Some(table) = mount.permits else {
            c.untabled.push(name);
            continue;
        };
        c.tabled += 1;
        for method in mount.descriptor.methods {
            if table.methods.iter().any(|p| p.method == method.method_name) {
                c.permitted += 1;
            } else {
                c.uncovered.push((name, method.method_name));
            }
        }
        for permit in table.methods {
            if !mount
                .descriptor
                .methods
                .iter()
                .any(|d| d.method_name == permit.method)
            {
                c.phantom.push((name, permit.method));
            }
        }
    }
    c
}

// ── Dry run ──────────────────────────────────────────────────────────────

/// "If enforcement were on right now, what would break?" — answered
/// statically by replaying every permit through the live engine with two
/// synthetic principals. Pure in-memory glob checks; cheap enough to run
/// at every boot.
#[derive(Debug, Default)]
pub struct DryRun {
    /// Methods a signed-in member may call.
    pub member_allowed: usize,
    /// `(service, method, reason)` a signed-in member may NOT call — the
    /// list that has to be empty (or deliberate) before enforcing.
    pub member_denied: Vec<(&'static str, &'static str, String)>,
    /// `(service, method)` callable with NO session at all — the
    /// `public/**` surface. Must be exactly the auth + permissions
    /// oracle methods.
    pub anonymous_allowed: Vec<(&'static str, &'static str)>,
    /// Methods that would be denied for EVERY principal because their
    /// service has a table but they are not in it.
    pub fail_closed: usize,
}

/// The user id used for the "ordinary signed-in user" probe. Any id works
/// — `RoleEngine::with_default_user_role("member")` maps unknown-but-
/// validated users to `member`, which is what every real user is today.
const PROBE_USER: &str = "dry-run-probe";

#[must_use]
pub fn dry_run(engine: &dyn PermissionEngine) -> DryRun {
    let member = Principal::User {
        user_id: PROBE_USER.to_owned(),
    };
    let anon = Principal::Anonymous;
    let mut out = DryRun::default();
    for mount in mounts() {
        let name = mount.descriptor.service_name;
        let Some(table) = mount.permits else { continue };
        for method in mount.descriptor.methods {
            let Some(permit) = table
                .methods
                .iter()
                .find(|p| p.method == method.method_name)
            else {
                out.fail_closed += 1;
                continue;
            };
            let resource = permit.coarse_resource();
            let action = Action::new(permit.action);
            match engine.check(&member, &resource, &action) {
                architect_permissions::Decision::Allow => out.member_allowed += 1,
                architect_permissions::Decision::Deny { reason } => {
                    out.member_denied.push((name, permit.method, reason));
                }
            }
            if engine.check(&anon, &resource, &action).allowed() {
                out.anonymous_allowed.push((name, permit.method));
            }
        }
    }
    out
}

/// Boot-time report — one summary line per org plus a warn line for every
/// gap. Called from [`crate::OrgAppState::new`] after the gate is built.
pub fn log_coverage(
    slug: &str,
    gate: &architect::permissions_gate::PermissionsGate,
    enforce: bool,
) {
    let c = coverage();
    let d = dry_run(gate.engine().as_ref());
    let mode = if enforce { "ENFORCING" } else { "observe-only" };
    tracing::info!(
        org = %slug,
        mode,
        services = c.services,
        tabled = c.tabled,
        methods = c.methods,
        permitted = c.permitted,
        member_allowed = d.member_allowed,
        anonymous_allowed = d.anonymous_allowed.len(),
        "permissions gate: {}/{} services have permit tables ({}/{} methods)",
        c.tabled,
        c.services,
        c.permitted,
        c.methods,
    );
    if !c.untabled.is_empty() {
        tracing::warn!(
            org = %slug,
            count = c.untabled.len(),
            services = %c.untabled.join(", "),
            "permissions gate: services with NO permit table — unchecked (UnlistedPolicy::Allow)",
        );
    }
    if !c.uncovered.is_empty() {
        tracing::warn!(
            org = %slug,
            count = c.uncovered.len(),
            methods = %join_pairs(&c.uncovered),
            "permissions gate: methods missing from their service's table — FAIL-CLOSED once enforcing",
        );
    }
    if !c.phantom.is_empty() {
        tracing::error!(
            org = %slug,
            count = c.phantom.len(),
            methods = %join_pairs(&c.phantom),
            "permissions gate: permits naming methods that do not exist (typo/rename) — dead rules",
        );
    }
    if !d.member_denied.is_empty() {
        tracing::warn!(
            org = %slug,
            count = d.member_denied.len(),
            methods = %d.member_denied.iter().map(|(s, m, _)| format!("{s}/{m}")).collect::<Vec<_>>().join(", "),
            "permissions gate dry-run: a signed-in member would be DENIED these — do not enforce until this is intentional",
        );
    }
    tracing::info!(
        org = %slug,
        methods = %join_pairs(&d.anonymous_allowed),
        "permissions gate dry-run: reachable without a session (public surface)",
    );
}

fn join_pairs(v: &[(&'static str, &'static str)]) -> String {
    v.iter()
        .map(|(s, m)| format!("{s}/{m}"))
        .collect::<Vec<_>>()
        .join(", ")
}

// ── Would-deny ledger + audit sink ───────────────────────────────────────

/// One distinct would-be denial, with a hit count.
#[derive(Debug, Clone, Default)]
pub struct DenyCount {
    pub count: u64,
    pub reason: String,
}

/// Bounded tally of everything the gate refused (or WOULD have refused, in
/// observe-only). This is the runtime half of the dry-run: the static
/// [`dry_run`] says what the rules imply, the ledger says what real
/// clients actually did.
#[derive(Debug, Default)]
pub struct DenyLedger {
    inner: Mutex<LedgerInner>,
}

#[derive(Debug, Default)]
struct LedgerInner {
    entries: HashMap<String, DenyCount>,
    /// Denials dropped because [`DenyLedger::CAP`] distinct reasons were
    /// already recorded.
    overflow: u64,
}

impl DenyLedger {
    /// Distinct reasons kept. Past this the tally still counts, it just
    /// stops growing — a gate log flooded by one broken client must not
    /// become a memory leak.
    pub const CAP: usize = 512;

    fn record(&self, reason: &str) {
        let Ok(mut g) = self.inner.lock() else { return };
        if let Some(e) = g.entries.get_mut(reason) {
            e.count += 1;
            return;
        }
        if g.entries.len() >= Self::CAP {
            g.overflow += 1;
            return;
        }
        g.entries.insert(
            reason.to_owned(),
            DenyCount {
                count: 1,
                reason: reason.to_owned(),
            },
        );
    }

    /// Snapshot, most-frequent first.
    #[must_use]
    pub fn snapshot(&self) -> (Vec<DenyCount>, u64) {
        let Ok(g) = self.inner.lock() else {
            return (Vec::new(), 0);
        };
        let mut v: Vec<DenyCount> = g.entries.values().cloned().collect();
        v.sort_by(|a, b| b.count.cmp(&a.count));
        (v, g.overflow)
    }
}

/// The org lane's [`AuditSink`]: makes observe-only mode *informative*.
///
/// The gate emits two events for an observe-only denial (see
/// `architect::permissions_gate`):
///
/// 1. the real decision — `allowed: false`, carrying principal, coarse
///    resource, action and the engine's reason (which embeds the
///    principal, e.g. `user:abc is not a member`);
/// 2. a marker — `resource: "gate"`, `action: "would-deny"`, **`allowed:
///    true`**, whose `reason` is the full deny message *including
///    `(service/method)`*.
///
/// The stock [`architect_permissions::TracingAudit`] logs (2) as
/// `info!("permission allow")` and **drops the reason entirely** — so the
/// one line that names the service and method never reaches the operator.
/// This sink logs it at WARN with the reason intact and tallies it in the
/// [`DenyLedger`], which is what makes the observe-only rollout auditable.
pub struct GateAudit {
    enforcing: bool,
    ledger: Arc<DenyLedger>,
    /// Role the org's engine assigns any validated user — mirrors
    /// `RoleEngine::with_default_user_role` in
    /// `crate::build_org_permissions_gate`. Logged so a deny line says
    /// which rule set was consulted (`AuditEvent` carries no role field).
    default_role: &'static str,
}

/// The marker resource/action the gate uses for observe-only would-denies.
const GATE_MARKER_RESOURCE: &str = "gate";
const GATE_MARKER_ACTION: &str = "would-deny";

impl GateAudit {
    #[must_use]
    pub fn new(enforcing: bool, ledger: Arc<DenyLedger>, default_role: &'static str) -> Self {
        Self {
            enforcing,
            ledger,
            default_role,
        }
    }
}

/// Wraps an [`IdentityResolver`] to put the auth outcome on the wide event.
///
/// `SessionIdentityResolver` returns `Principal::Anonymous` for two very
/// different situations — **the client sent no token at all**, and **the
/// client sent a token the session store rejected** — and nothing
/// downstream can tell them apart. That distinction is the difference
/// between "the UI never signed in" and "sessions are expiring", and its
/// absence is exactly what made a real not-signed-in incident slow to
/// diagnose: every RPC logged `principal=anonymous` and the logs could
/// not say why.
///
/// A decorator, not a change to `architect_auth`, so the framework keeps
/// no opinion about telemetry. Records the *shape* of the credential,
/// never the credential:
///   `auth.principal_kind`   user | anonymous
///   `auth.user_id`          (when resolved)
///   `auth.token_presented`  true | false
///   `auth.outcome`          resolved | rejected | absent
/// It also carries `org.slug`. The org is only known at the WebSocket
/// upgrade, which is a different span from the RPCs that ride the socket
/// afterwards — but identity resolution runs per RPC and is built per
/// org, so it is the natural place to stamp it. Without this a wide
/// event cannot answer "which org", which is the first question in a
/// multi-tenant system.
pub struct AuditedIdentityResolver<R> {
    inner: R,
    slug: String,
}

impl<R> AuditedIdentityResolver<R> {
    pub fn new(inner: R, slug: impl Into<String>) -> Self {
        Self {
            inner,
            slug: slug.into(),
        }
    }
}

/// Accept a home-org token for THIS org — but only with a membership row.
///
/// The org's own store answers first, so nothing about single-org
/// behaviour changes and a token issued here keeps working after the
/// home org is gone. Only when that yields no user do we ask the home
/// org, and a home principal is admitted **only** if
/// `memberships.role_for(user, slug)` returns a row.
///
/// That row is the entire fence. Before this existed, a `codywright`
/// token was simply meaningless to `cbu` — the wrong database, no
/// decision to get wrong. Now it is meaningful everywhere on the server
/// and the row is what says no, which is why a missing row must fail
/// closed rather than fall through to `DEFAULT_ORG_ROLE`. See
/// one account per server.
pub struct HomeFallbackResolver<R, H> {
    own: R,
    home: H,
    memberships: std::sync::Arc<crate::memberships::Memberships>,
    slug: String,
}

impl<R, H> HomeFallbackResolver<R, H> {
    pub fn new(
        own: R,
        home: H,
        memberships: std::sync::Arc<crate::memberships::Memberships>,
        slug: impl Into<String>,
    ) -> Self {
        Self {
            own,
            home,
            memberships,
            slug: slug.into(),
        }
    }
}

impl<R: IdentityResolver, H: IdentityResolver> IdentityResolver for HomeFallbackResolver<R, H> {
    fn resolve<'a>(&'a self, bearer_token: Option<&'a str>) -> BoxIdentityFuture<'a> {
        Box::pin(async move {
            use architect_telemetry::wide;

            let own = self.own.resolve(bearer_token).await;
            if matches!(own, Principal::User { .. }) {
                return own;
            }
            let Principal::User { user_id } = self.home.resolve(bearer_token).await else {
                return Principal::Anonymous;
            };
            // A home principal exists. Membership decides.
            let Ok(uuid) = user_id.parse::<uuid::Uuid>() else {
                // Ids are uuids everywhere in architect-auth; a token that
                // resolves to something else is not a shape we admit
                // across orgs.
                wide::set("auth.cross_org", "unparsable_user_id");
                return Principal::Anonymous;
            };
            match self.memberships.role_for(uuid, &self.slug).await {
                Ok(Some(m)) => {
                    wide::set("auth.cross_org", "member");
                    wide::set(
                        "auth.membership_role",
                        m.role.unwrap_or_else(|| "(member)".into()),
                    );
                    Principal::User { user_id }
                }
                Ok(None) => {
                    // Signed in, and not a member here. ONE warn line:
                    // this is a refusal, and refusals are alertable.
                    wide::set("auth.cross_org", "not_a_member");
                    tracing::warn!(
                        org.slug = self.slug,
                        "cross-org: home principal has no membership row for this org"
                    );
                    Principal::Anonymous
                }
                Err(e) => {
                    // Fail closed: an unreadable membership table must not
                    // become "everyone is a member".
                    wide::set("auth.cross_org", "lookup_failed");
                    tracing::warn!(
                        org.slug = self.slug,
                        error = %e,
                        "cross-org: membership lookup failed — refusing"
                    );
                    Principal::Anonymous
                }
            }
        })
    }
}

/// The peer admission model, re-exported from where it now lives.
///
/// It was defined here, which put it out of reach of the callers who need
/// it most: a device serving content to another device cannot depend on
/// the server binary to find out who it may talk to. It moved to
/// `files::peer` with the rest of the peering feature; these keep the
/// paths this module's own tables and docs refer to.
pub use files::peer::{HOST_BEARER_PREFIX, HostEngine, HostResolver};

impl<R: IdentityResolver> IdentityResolver for AuditedIdentityResolver<R> {
    fn resolve<'a>(&'a self, bearer_token: Option<&'a str>) -> BoxIdentityFuture<'a> {
        Box::pin(async move {
            use architect_telemetry::wide;

            wide::set("org.slug", self.slug.clone());
            let presented = bearer_token.is_some_and(|t| !t.is_empty());
            wide::set("auth.token_presented", presented);

            let principal = self.inner.resolve(bearer_token).await;
            match &principal {
                Principal::User { user_id } => {
                    wide::set("auth.principal_kind", "user");
                    wide::set("auth.user_id", user_id.clone());
                    wide::set("auth.outcome", "resolved");
                }
                _ => {
                    wide::set("auth.principal_kind", "anonymous");
                    // THE field. Same principal, opposite root causes.
                    wide::set(
                        "auth.outcome",
                        if presented { "rejected" } else { "absent" },
                    );
                }
            }
            principal
        })
    }
}

impl AuditSink for GateAudit {
    /// Record a gate decision onto the request's wide event.
    ///
    /// This used to emit a log line per decision. One page load produced
    /// ~24 of them — the exact scatter the wide-event pattern exists to
    /// kill: unaggregatable, uncorrelated, and each one missing the
    /// context (which RPC? which org? whose session?) that would have
    /// made it actionable. The RPC span already holds that context, so
    /// the decision belongs ON it.
    ///
    /// Fields, not prose, so a single query answers "who is being denied
    /// what, and would enforcing break them":
    ///   `perm.decision`  allow | deny | would_deny
    ///   `perm.mode`      enforcing | observe-only
    ///   `perm.principal` / `perm.resource` / `perm.action` / `perm.reason`
    ///
    /// A denial still emits ONE log line, because a denial is the kind of
    /// thing you want to alert on without querying traces — but it is now
    /// the only line, and it carries the full tuple.
    fn record(&self, e: AuditEvent) {
        use architect_telemetry::wide;

        let reason = e.reason.unwrap_or_default();
        let mode = if self.enforcing {
            "enforcing"
        } else {
            "observe-only"
        };
        wide::set("perm.mode", mode);
        wide::set("perm.default_role", self.default_role);

        if e.resource == GATE_MARKER_RESOURCE && e.action == GATE_MARKER_ACTION {
            // Observe-only marker: the ONLY event carrying service/method.
            self.ledger.record(&reason);
            wide::set("perm.decision", "would_deny");
            wide::set("perm.reason", reason.clone());
            tracing::warn!(
                target: "task_server::permissions",
                mode = "observe-only",
                default_role = self.default_role,
                reason = %reason,
                "permissions gate: WOULD DENY (allowed through — enforcement is off)",
            );
            return;
        }

        wide::set_display("perm.principal", &e.principal);
        wide::set_display("perm.resource", &e.resource);
        wide::set_display("perm.action", &e.action);

        if !e.allowed {
            self.ledger.record(&reason);
            wide::set("perm.decision", "deny");
            wide::set("perm.reason", reason.clone());
            // In observe-only this pairs with the marker line above; when
            // enforcing it is the whole record (the gate emits no marker),
            // so it carries the principal/resource/action explicitly.
            tracing::warn!(
                target: "task_server::permissions",
                mode,
                principal = %e.principal,
                default_role = self.default_role,
                resource = %e.resource,
                action = %e.action,
                reason = %reason,
                "permissions gate: DENY",
            );
            return;
        }

        // Allows are the overwhelming majority. They ride the wide event
        // only — a log line per allowed call is pure noise, and the span
        // already records that the call happened at all.
        wide::set("perm.decision", "allow");
    }
}

// ── Operator report ──────────────────────────────────────────────────────

/// The JSON body of `GET /server/permissions` — everything an operator
/// needs to answer "what breaks if I set `TASK_ENFORCE_PERMISSIONS=1`".
#[must_use]
pub fn report_json(
    enforce: bool,
    engine: &dyn PermissionEngine,
    ledger: &DenyLedger,
) -> serde_json::Value {
    let c = coverage();
    let d = dry_run(engine);
    let (denies, overflow) = ledger.snapshot();
    serde_json::json!({
        "mode": if enforce { "enforcing" } else { "observe-only" },
        "enforce_env": "TASK_ENFORCE_PERMISSIONS",
        "coverage": {
            "services": c.services,
            "tabled": c.tabled,
            "untabled": c.untabled,
            "methods": c.methods,
            "permitted": c.permitted,
            "uncovered": c.uncovered.iter().map(|(s, m)| format!("{s}/{m}")).collect::<Vec<_>>(),
            "phantom": c.phantom.iter().map(|(s, m)| format!("{s}/{m}")).collect::<Vec<_>>(),
            "complete": c.is_complete(),
        },
        "dry_run": {
            "member_allowed": d.member_allowed,
            "member_denied": d.member_denied.iter()
                .map(|(s, m, r)| serde_json::json!({ "method": format!("{s}/{m}"), "reason": r }))
                .collect::<Vec<_>>(),
            "anonymous_allowed": d.anonymous_allowed.iter()
                .map(|(s, m)| format!("{s}/{m}")).collect::<Vec<_>>(),
            "fail_closed": d.fail_closed,
        },
        "observed_denials": {
            "distinct": denies.len(),
            "dropped_over_cap": overflow,
            "top": denies.iter().take(50)
                .map(|d| serde_json::json!({ "count": d.count, "reason": d.reason }))
                .collect::<Vec<_>>(),
        },
    })
}

/// Human-readable coverage summary — `task doctor`'s permissions check.
/// Static (no server, no network): it folds the same tables the server
/// would install.
#[must_use]
pub fn coverage_summary() -> String {
    let c = coverage();
    let mut out = format!(
        "Permissions coverage: {}/{} services tabled, {}/{} methods permitted",
        c.tabled, c.services, c.permitted, c.methods
    );
    if !c.untabled.is_empty() {
        out.push_str(&format!(
            "\n  UNGATED services ({}): {}",
            c.untabled.len(),
            c.untabled.join(", ")
        ));
    }
    if !c.uncovered.is_empty() {
        out.push_str(&format!(
            "\n  FAIL-CLOSED methods once enforcing ({}): {}",
            c.uncovered.len(),
            join_pairs(&c.uncovered)
        ));
    }
    if !c.phantom.is_empty() {
        out.push_str(&format!(
            "\n  DEAD permits (method not on the descriptor) ({}): {}",
            c.phantom.len(),
            join_pairs(&c.phantom)
        ));
    }
    if c.is_complete() {
        out.push_str("\n  OK — every mounted service and method is covered.");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every mounted service has a table, every descriptor method has a
    /// permit, and no permit names a method that does not exist. This is
    /// the guard that keeps the 2/71 gap from reappearing: add a service
    /// to `org_layer_router` without a table and this fails.
    #[test]
    fn coverage_is_complete() {
        let c = coverage();
        assert!(
            c.untabled.is_empty(),
            "mounted services with no permit table: {:?}",
            c.untabled
        );
        assert!(
            c.uncovered.is_empty(),
            "methods missing a permit (fail-closed once enforcing): {:?}",
            c.uncovered
        );
        assert!(
            c.phantom.is_empty(),
            "permits naming non-existent methods: {:?}",
            c.phantom
        );
        assert_eq!(c.services, c.tabled);
    }

    /// The dry-run against the real org engine: an ordinary signed-in
    /// member must be able to call EVERYTHING, and only the deliberate
    /// `public/**` surface may be reachable anonymously. If this fails,
    /// flipping `TASK_ENFORCE_PERMISSIONS=1` would lock users out.
    #[test]
    fn member_may_call_everything_and_public_surface_is_minimal() {
        let engine = crate::org_permission_engine();
        let d = dry_run(engine.as_ref());
        assert!(
            d.member_denied.is_empty(),
            "a signed-in member would be denied: {:?}",
            d.member_denied
        );
        assert_eq!(d.fail_closed, 0);
        let public: Vec<&str> = d
            .anonymous_allowed
            .iter()
            .map(|(s, _)| *s)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        // `FederationService` is here for its three `*_offered`
        // methods and nothing else. A receiver on another server has no
        // session with this org and never will, so the coarse gate can
        // only answer "not a member"; what authenticates the call is
        // the offer secret, checked at one chokepoint so a withdrawal
        // binds on the next call of any kind. The other six methods on
        // that service — `offer`, `withdraw`, `accept`, `forget` and
        // the two reads — stay on `files/**`.
        assert_eq!(
            public,
            vec!["AuthService", "FederationService", "PermissionsService"],
            "the anonymous surface changed — every entry must be a service \
             that authenticates its own callers",
        );
    }

    /// Every mount's `plugin` is a real `task_plugin::CATALOG` id — a
    /// typo here would silently unmount the service for every org (an
    /// unknown id is never in a resolved `PluginSet`).
    #[test]
    fn every_mount_plugin_is_a_known_catalog_id() {
        for mount in mounts() {
            assert!(
                task_plugin::find(mount.plugin).is_some(),
                "{} names unknown plugin `{}`",
                mount.descriptor.service_name,
                mount.plugin
            );
        }
    }

    /// The filtered view: everything with no deny-list; core survives
    /// any deny-list; a denied plugin's mounts (and only those) drop.
    #[test]
    fn mounts_for_filters_by_plugin() {
        use task_plugin::{PluginChoice, PluginSet};
        let all = PluginSet::resolve(None);
        assert_eq!(mounts_for(&all).len(), mounts().len());

        let no_mealplan =
            PluginSet::resolve(Some(&PluginChoice::Disabled(vec!["mealplan".into()])));
        let filtered = mounts_for(&no_mealplan);
        assert!(filtered.iter().all(|m| m.plugin != "mealplan"));
        let dropped = mounts().iter().filter(|m| m.plugin == "mealplan").count();
        assert!(dropped > 0, "the mealplan plugin owns mounts");
        assert_eq!(filtered.len(), mounts().len() - dropped);
    }

    /// No `admin` permits: nobody holds the `owner` role on the org lane
    /// today (no `set_member` calls), so an admin permit is a lockout.
    #[test]
    fn no_admin_permits() {
        for mount in mounts() {
            let Some(table) = mount.permits else { continue };
            for p in table.methods {
                assert_ne!(
                    p.action,
                    Action::ADMIN,
                    "{}/{} requires `admin`, which no user holds",
                    table.service,
                    p.method
                );
            }
        }
    }

    #[test]
    fn ledger_counts_and_caps() {
        let l = DenyLedger::default();
        l.record("a");
        l.record("a");
        l.record("b");
        let (v, over) = l.snapshot();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].count, 2);
        assert_eq!(over, 0);
        for i in 0..DenyLedger::CAP {
            l.record(&format!("r{i}"));
        }
        let (v, over) = l.snapshot();
        assert_eq!(v.len(), DenyLedger::CAP);
        assert!(over > 0);
    }
}
