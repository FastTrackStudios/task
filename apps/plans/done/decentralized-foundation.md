# Decentralized, Knowledge-First, Offline-First Foundation

This document pins down the architecture before more code lands. It
supersedes every earlier scoping doc in `plans/`. Subsequent work
follows the implementation phases at the bottom.

## 1. Vision (in one paragraph)

We're building a **self-hosted, decentralized, offline-first
collaborative knowledge platform**. The core is an
Obsidian/Logseq-compatible Knowledge layer — pages, blocks,
frontmatter, wiki-links, backlinks. Every other "feature" people
expect (tasks, projects, people, clients, calendar, fitness logs,
recipes, audio sessions, …) is **a `kind:` frontmatter convention
+ a Bases-style custom view over Knowledge pages**, not its own
data type. Servers are **per-org**; a typical user runs their own
server hosting their personal life + side projects + a music
studio + a software company, and connects as an *employee* to a
separate company's server, and as an *anonymous share-link
recipient* to a client's server. The client app is the
federation layer — it talks to N servers, each sovereign over
its data, and merges views client-side. Markdown round-trip
(import + export) is a gateway feature, not the source of truth;
the CRDT doc is canonical.

## 2. Actors and roles

Five distinct relationships any user can have with any server.
The capability layer (§5) is what makes these all expressible:

| Role | Example | Granted by | Scope |
|---|---|---|---|
| **Owner** | You on your personal server | Server bootstrap | Everything |
| **Admin** | You on your studio server | Owner | Org-wide, can manage members |
| **Member** | You as an employee | Admin invitation | Org-scoped; role-limited (see architect-auth) |
| **Collaborator** | A client working on one project on your server | Project admin | One project only; role limited within it |
| **Anonymous via share link** | A client downloading their files; an external reviewer | Share-link token | Single resource, single scope, time-limited |

A user has **N identities** — one per server they connect to.
Identities are not federated. The client app is the only place
data ever crosses orgs.

## 3. Architecture overview

```
                          ┌─────────────────────────────┐
                          │       Client (Dioxus)       │
                          │   Multi-server federation   │
                          └──┬─────────┬─────────┬──────┘
                  per-server │         │         │
                   identity  │         │         │ per-server
                  + tokens   │         │         │ vox sessions
                             ▼         ▼         ▼
                ┌────────────────┐  ┌────────┐  ┌──────────────┐
                │ personal.       │  │ studio │  │ acme-corp    │
                │ cody.dev        │  │ .fts.dev│  │ .com         │
                │ (owner)         │  │ (admin)│  │ (employee)   │
                └────────────────┘  └────────┘  └──────────────┘
                Each server is sovereign over its own org's data.
```

Each server (one per org):

```
┌──────────────────────────────────────────────────────────────┐
│                     task-server (per org)                    │
│                                                              │
│  ┌─ architect-auth ────────┐    ┌─ Capabilities ─────────┐   │
│  │  Users, sessions,       │ →  │  Decode token from WS  │   │
│  │  orgs, roles, teams,    │    │  Resolve to user OR    │   │
│  │  invitations, OAuth     │    │  share-link scope      │   │
│  │  → vox services         │    │  Attach to req context │   │
│  └─────────────────────────┘    └────────────────────────┘   │
│                                              │               │
│                                              ▼               │
│  ┌─ vox /vox endpoint ──────────────────────────────────┐   │
│  │   subscribe(doc_id, since, output: Tx<UpdateBytes>) │   │
│  │   apply_update(doc_id, bytes)                       │   │
│  │   per-entity Repo dispatchers (read paths)          │   │
│  │   AttachmentService (upload/download presigned URL) │   │
│  │   ShareService (create/list/revoke share links)     │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                              │
│  ┌─ Doc registry (HashMap<DocId, Arc<CrdtDoc>>) ────────┐   │
│  │   org-wide vault doc      (members, schemas,         │   │
│  │                            workflows, people, etc.)  │   │
│  │   project/<uuid> doc × N  (one per project)          │   │
│  │   page/<uuid> doc × N     (one per shared Knowledge  │   │
│  │                            page that's heavily       │   │
│  │                            edited — defer to Phase 3)│   │
│  └─────────────────────────────────────────────────────┘   │
│                                                              │
│  ┌─ Persistence ──────────────────────────────────────────┐ │
│  │   SeaORM/SQLite per server (the org's data)           │ │
│  │   Object store for attachments (filesystem v0, S3 v1) │ │
│  └────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘
```

## 4. Knowledge as the data substrate

**Everything stored on a server is Knowledge.** No per-feature
data types. The Knowledge proto already on main defines:

- `Vault` — a workspace boundary
- `Folder` — first-class for empty-folder persistence
- `Page` — one markdown/canvas/base file with frontmatter
- `Block` — one paragraph/heading/list-item with stable id
- `KnowledgeTag` — global tag registry
- `Base` — Obsidian `.base` filtered/sorted view

The `refs.rs` types already model the relations:

- `LinkRef` — `[[Page]]` / `[[Page#heading]]` / `[[Page#^block]]`
- `EmbedRef` — `![[Page]]`
- `TagRef` — `#nested/tag`
- `EntityRef` — `[[entity://kind/uuid]]` (typed)
- `BlockRef` — `((block-id))`

### Custom workflows via `kind:` frontmatter

A Person is a Page with `kind: person`. A Project is a Page with
`kind: project`. A Workout is a journal Page with `kind: workout`.
A Recording Session is a Page with `kind: recording_session`. A
Workflow Definition (org admin-authored) is a Page with `kind:
workflow_template`.

Pages reference each other via wiki links:

```yaml
---
kind: project
client: "[[Acme Corporation]]"
lead: "[[Cody]]"
members: ["[[Alice]]", "[[Bob]]"]
status: in-progress
budget: 25000
acl:
  read: ["@org-members"]
  write: ["[[Cody]]", "[[Alice]]"]
  share_links: [4f3a..., 7b2c...]
---
# Q4 Album Mix

Tracking the mix sessions for [[Acme Corporation]]'s Q4 album.
- [[Recording Session 2026-11-12]]
- [[Recording Session 2026-11-19]]
```

When the user clicks `[[Acme Corporation]]`, the client navigates
to that page. The Acme page sees backlinks from every project
mentioning it — automatically.

### Three vault tiers per server

Server holds three Knowledge vaults:

- **`vault://org`** — global org reference data. Members,
  schemas, glossary, workflow templates, people directory,
  org-wide policies. Every member reads; admins write.
- **`vault://comms`** — centralized communication. Chat threads,
  email threads, voice transcripts. Each thread is its **own
  sub-doc** (`comms/thread/<uuid>`) — busy orgs have 50k+
  threads, so per-thread docs not one mega-doc. The vault root
  page is an index (thread metadata, participants, last
  message-at). ACL lives on each thread page; private DMs are
  participant-only, public channels are org-readable.
- **`vault://project/<uuid>`** — one per project. Project members
  read+write per ACL. Pages here can wiki-link freely across all
  three vaults — to people in `org`, to messages in `comms`, to
  pages in other projects.

A "person" lives in the org vault. An email thread lives in the
comms vault. A "project task" lives in the project vault. A
wiki-link from a project page to an email message
(`[[Email: Acme renewal#^msg-abc]]`) crosses vault boundaries —
resolved at query time, all three vaults on the same server.

### Why centralized comms (not per-project)

Communication is **inherently cross-cutting**:

- An email arrives in the inbox before anyone knows which project
  it belongs to (or whether it belongs to a project at all).
- One thread often references multiple projects, or branches into
  a new one over time.
- Personal emails / DMs aren't "about" any project.
- Mail clients (Nextcloud Mail, Apple Mail, Gmail) and chat
  platforms (Slack, Discord) already model storage this way: one
  warehouse per-user/per-org, tagged + searched, never moved into
  "project folders."

Projects backlink to specific messages when relevant. The thread
stays in `vault://comms`. Backlinks emerge automatically:
opening an email-thread page shows "Referenced by these 3
projects."

### Bridges write to vault://comms

- **IMAP bridge** → `kind: email_thread` pages, one block per
  message. Pulls from Nextcloud Mail's IMAP backend (or any
  IMAP).
- **Slack / Discord / etc. bridges** → `kind: chat_thread`
  pages, one block per message. Webhook-driven.
- **Voice memo / transcript ingestion** (later) → `kind:
  transcript` pages.

Multiple sources, one target vault.

### Agents synthesize across vaults

```
Agent runtime
   reads:  vault://comms threads
           vault://org for participant context
           vault://project/<id> for current state
   thinks: "this Acme email mentions the renewal deadline"
   writes: kind: decision block on the project page, with
           derived_from: [[Email: Acme renewal#^msg-abc]]
           (cross-vault block ref, resolves via wiki-link index)
```

Synthesis lives in the project vault. Raw comms stay where they
are. Both linked. **Audit trail + curated surface side by
side.**

### Federation gives unified inbox

When a client federates across N servers:

```
Client connects to:
├── personal.cody.dev         vault://comms = personal email
├── studio.fasttrackaudio.com vault://comms = studio Slack + email
└── acme-corp.com             vault://comms = work email (employee scope)
```

A unified-inbox view queries `kind: email_thread` + `kind:
chat_thread` across all three servers' comms vaults. One screen,
three sovereign backends. This is where federation pays off
operationally.

### Custom views

A "Tasks" view is a Bases query: pages with `kind: task`,
filtered by status. A "Clients directory" is `kind: client`
sorted by name. A "This week" view is journal pages with date in
the last 7 days. Each is a small Rust component that consumes a
Bases query result and renders it (list / kanban / calendar /
gallery / map).

**Zero per-feature data crates.** Adding a new entity type is
adding a Knowledge page with a `kind:` frontmatter convention.

## 5. Capability tokens

### Goals

- Anonymous share links: token-in-URL grants scoped access with
  no account.
- Authenticated sessions: still bind to architect-auth users +
  org/role.
- Per-resource scope: a token grants access to **one resource
  (typically a project doc) with one scope** (read / read-write /
  read-attachments-only).
- Time-bound and revocable.
- No need for the recipient to register.

### Shape

```rust
// Wire format: signed by the server's secret key.
struct CapabilityToken {
    /// Token id; the server maintains a revocation table.
    id: Uuid,
    /// The doc this token gates access to.
    doc_id: DocId,
    /// What operations are allowed.
    scope: TokenScope,
    /// Issuer (server) — important when a client roams between
    /// servers and tokens accidentally get sent to the wrong one.
    issuer: ServerId,
    /// Optional user binding. If `Some`, this token only works
    /// for that auth user (a "magic link" to a private dashboard).
    /// If `None`, anonymous — anyone with the bytes is authorized.
    subject: Option<UserId>,
    issued_at: i64,
    expires_at: Option<i64>,
}

enum TokenScope {
    /// Read entities + subscribe to doc updates.
    Read,
    /// Read + apply_update (full collab).
    ReadWrite,
    /// Read only the attachments collection; no doc subscribe.
    /// Used for "client download your files" links.
    AttachmentsOnly,
    /// Custom: per-entity-type allow/deny lists.
    Custom { ops: Vec<String> },
}
```

### Encoding

**Use vox/Facet-encoded bytes + Ed25519 signature**, not JWT.

- We already have facet binary encoding everywhere.
- JWT's base64-JSON overhead buys nothing for non-browser
  scenarios.
- A token is a short opaque string (the facet bytes,
  base64url-encoded for URL safety) appended to the share URL.

`https://studio.example.com/share/<base64url-token>`

### Transport

Token rides as `?cap=<base64url>` on the vox WS handshake URL.
Server middleware parses + verifies + attaches a
`Capability { user, doc, scope }` to the request context. Every
vox dispatcher checks the cap before serving.

For architect-auth-issued sessions, the same query parameter
carries a session token; capability middleware tries token
formats in order until one verifies.

### Revocation

Server keeps a `RevokedTokens` table (token_id, revoked_at).
Capability middleware short-circuits if the token id is in the
table. Project admins call `ShareService::revoke(token_id)` to
invalidate a link.

## 6. ACL resolution

Each project doc carries ACL frontmatter on its root page
(`README.md` or equivalent inside the project vault). The
resolver runs on every authenticated call:

```rust
fn authorize(req: &Request, doc: &CrdtDoc) -> AuthDecision {
    match req.capability {
        Capability::ShareToken { scope, doc_id, .. }
            if doc_id == doc.id => scope.allows(req.method),

        Capability::AuthUser { user_id, .. } => {
            let acl = doc.read_root_acl();  // the YAML frontmatter
            // Resolve wiki-links to auth user ids via the
            // server's wiki-link index. Some [[Names]] won't
            // have an `auth_user_id` — they're just contacts,
            // not accounts. Those entries don't grant access.
            for entry in acl.write {
                if let Some(uid) = resolve_wikilink_to_user(&entry) {
                    if uid == user_id { return AuthDecision::Allow(WriteScope) }
                }
            }
            // Fallback: org-level role.
            if user_id.is_org_member() && acl.read_anyone_in_org {
                AuthDecision::Allow(ReadScope)
            } else {
                AuthDecision::Deny
            }
        }

        Capability::None => AuthDecision::Deny,
    }
}
```

### Wiki-link → user resolution

When a Person page in the org vault has frontmatter
`auth_user_id: <uuid>`, that page is **bound** to an
architect-auth user. ACL entries like `"[[Cody]]"` resolve to
that user via a server-maintained index
`page_basename → (page_id, auth_user_id)`.

A Person page without `auth_user_id` is just a contact — they
appear in directories, can be wiki-linked, can be tagged in
project members, but **don't grant any access** because no
account exists to grant access TO.

This is the "promote a contact to a collaborator" flow:

1. Admin creates `[[Jane Doe]]` page (kind: person, email: jane@…).
2. Admin invites Jane via architect-auth: `CreateInvitation`.
3. Jane accepts, gets an `auth_user_id`.
4. Admin (or server, automatically) writes `auth_user_id: <jane's uuid>` to her page's frontmatter.
5. Now `"[[Jane Doe]]"` in any project ACL grants Jane access.

## 7. Doc-id transport

### Naming

Doc IDs are **server-local**. The server already implies the org.
Format: `<resource-type>/<uuid>` or named like `vault/org` for the
single org vault.

| Doc id | Contents |
|---|---|
| `vault/org` | Org-wide reference: people, schemas, workflows, glossary |
| `vault/comms` | Comms vault index: thread metadata, participants, last-message-at. The actual threads are sub-docs (see next row). |
| `comms/thread/<uuid>` | One chat or email thread — one block per message. ACL on the thread page determines who can read. |
| `project/<uuid>` | One project — its tasks, milestones, decisions, attachments, ACL |
| `user/<uuid>` | Per-user private state (inbox state, saved views) — *Phase 4* |

### WorkspaceSync trait, revised

```rust
#[vox::service]
trait WorkspaceSync {
    /// Subscribe to one doc. `since` is the peer's Loro
    /// VersionVector — server sends only the delta. First message
    /// is either a `Snapshot` (if `since == empty`) or an
    /// incremental update batch.
    async fn subscribe(
        &self,
        doc_id: DocId,
        since: VersionVectorBytes,
        output: Tx<UpdateBytes>,
    ) -> Result<(), SyncError>;

    /// Push local commits to a specific doc.
    async fn apply_update(
        &self,
        doc_id: DocId,
        bytes: UpdateBytes,
    ) -> Result<(), SyncError>;

    /// List doc IDs the caller can see (filtered by capability).
    async fn list_docs(&self) -> Result<Vec<DocSummary>, SyncError>;
}
```

`DocId` is the typed string `(kind, uuid)` pair.

Server's `HashMap<DocId, Arc<CrdtDoc>>` is the registry. LRU
eviction of cold docs to keep RAM bounded. Persistence loads on
first subscribe.

### Per-entity sync (still desirable, deferred)

Inside one project doc, a peer might only care about `task` blocks
and not `recording_session` blocks. Per-entity-kind sync streams
are an optimization layered on top of doc-id subscribe. Defer to
Phase 5.

## 8. Client federation

The client owns a `ServerRegistry`:

```rust
struct ServerRegistry {
    servers: Vec<ServerEntry>,
}

struct ServerEntry {
    url: ServerUrl,                // wss://studio.example.com/vox
    name: String,                  // "FastTrackStudio"
    identity: Option<AuthIdentity>,// signed-in user, if any
    capabilities: Vec<Capability>, // share tokens this client holds
    session: Option<LiveSession>,  // live vox session if connected
}
```

On startup, the client tries to reconnect to every server in the
registry. Each gets its own `LiveSession` (the pattern we
already shipped). UI views fan queries out to every connected
server, render the union.

**Anonymous mode.** The client can run with zero
`ServerEntry::identity`s — only `capabilities` (share-link tokens).
The window opens at `https://server.example.com/share/<token>`
and the client extracts the token, registers the URL, and opens
a session with that capability.

### Cross-server wiki links (deferred)

A future extension: `[[work:Cody]]` resolves to "server tagged
`work` in my registry, page named `Cody`". Resolution is purely
client-side; servers never know about each other. Phase 6+.

## 9. Attachments

Large media (audio masters, video deliverables, RAW photos, DAW
project files) doesn't belong in Loro. Object store, referenced
from Knowledge pages.

### Service

```rust
#[vox::service]
trait AttachmentService {
    /// Returns a presigned URL the client uploads the bytes to.
    /// The capability check has already verified the caller can
    /// write to this project.
    async fn initiate_upload(
        &self,
        project_id: Uuid,
        filename: String,
        content_hash: String,
        size_bytes: u64,
    ) -> Result<UploadTicket, AttachmentError>;

    /// Returns a short-lived presigned URL for download. Capability
    /// checks the caller's access to the project.
    async fn get_download_url(
        &self,
        attachment_id: Uuid,
    ) -> Result<DownloadUrl, AttachmentError>;
}
```

### Backend tiers

| v0 | Local filesystem under `${TASK_DATA_DIR}/attachments/<project_id>/<hash>`. Server-signed URLs valid for 5 min, served by axum at `/files/...`. |
| v1 | S3-compat (MinIO, Backblaze B2, AWS). Same RPC surface; server hands out presigned S3 URLs. |
| v2 | Content-addressed deduplication (same hash across projects = one blob). |

### Loro entity reference

A Block with `kind: attachment` carries `attachment_id: <uuid>` +
`filename`, `content_hash`, `size_bytes`, `mime_type` in frontmatter.
The actual bytes are NOT in Loro. The block is just a pointer.

### "Send files to a client without an account"

1. Project admin creates a share link, scope =
   `AttachmentsOnly`, expires in 30 days.
2. Client opens the URL.
3. Anonymous mode: the client subscribes to the project's
   attachments listing, downloads what they need.
4. Project admin can revoke the link at any time.

## 10. Markdown export/import

**Not real-time.** Two CLI/UI commands:

- `task export --server <url> --project <id> --format obsidian
  --out <dir>` writes a full Obsidian-compatible vault to disk.
  `.md` files for pages, `.obsidian/` config, attachments in
  `_attachments/`.
- `task import --server <url> --path <dir>` reads markdown +
  frontmatter, creates Knowledge entities, pushes them via
  `apply_update`.

Round-trip stability is the bar: `export → import → export`
must produce byte-identical output for unchanged content.

No filesystem watcher, no real-time conflict resolution between
disk and Loro, no `inotify`. If you want to edit in Obsidian
between sync sessions, you export, edit, import. The CRDT
fixes any merge weirdness on import.

## 11. Cross-cutting design decisions

Quick decisions on the small stuff so we don't relitigate later.

| Question | Decision | Why |
|---|---|---|
| Doc-id naming | `<kind>/<uuid>`, server-local | Server URL already implies org |
| Capability format | Facet bytes + Ed25519 sig | Reuses our tooling; JWT is browser-centric overhead |
| Token in transport | `?cap=<base64url>` on WS URL | Standard; capability middleware reads it |
| ACL location | Project-doc root page frontmatter | Versioned + collaborative + mergeable |
| Person ↔ auth user link | `auth_user_id` frontmatter on Person page | Optional binding; Person pages exist without accounts |
| Anonymous peer id in Loro | `share-link-<token-id>` | Stable, but identifiable in history |
| Wiki link resolution | Server-side index `basename → page_id` | O(1) lookup, rebuilt on commit |
| Cross-vault refs in one server | Allowed; resolved at query time | Both vaults on the same server, same identity check |
| Cross-server refs (`[[work:Cody]]`) | Client-side resolution only | Server never knows about other servers |
| LoroDoc per project vs per page | Project for now; per-page if needed | Per-project is the natural sharing unit; pages are mostly small |
| Schema versioning | Facet `#[facet(default)]` on new fields | We're already on the vox schema-evolution path |
| ACL conflict resolution | Loro's default (last-write per peer) | Admins should coordinate; tombstones if needed later |
| Member impersonation | Defer to architect-auth's `ImpersonateUser` | Already implemented |

## 12. What we keep from existing work

**Everything we've built on `thin-vertical-slice` is reusable:**

- CRDT-over-vox transport (`WorkspaceSync`, `Tx<UpdateBytes>`)
- `entity_crdt!` macro
- Server-side broadcast + subscribe/apply_update split
- Stress + browser test infrastructure
- Knowledge proto + crdt copied in (one entity migration in flight)

**What needs to change:**

- `WorkspaceSync` gains a `doc_id` parameter (Phase 1).
- Server's `AppState` holds `HashMap<DocId, Arc<CrdtDoc>>` instead
  of a single workspace doc.
- A capability middleware lives in front of every vox dispatcher.
- The project/task `*RepoLoro`s we built migrate **into** the
  Knowledge model — Project becomes a `kind: project` page, Task
  becomes a `kind: task` block. The standalone `project-proto` /
  `project-crdt` crates eventually become legacy.

## 13. Implementation phases

Each phase is independently testable + commitable. No phase
should depend on a future phase.

### Phase 1: Doc-id transport (foundation)

- Change `WorkspaceSync::{subscribe, apply_update}` to take
  `doc_id: DocId`.
- Server's `HashMap<DocId, Arc<CrdtDoc>>` with LRU eviction.
- Persistence schema: add `doc_id` column to snapshot + update
  tables.
- Client `LiveSession::open(server_url, doc_id)` opens a session
  scoped to one doc.
- **Test**: two peers subscribe to two different doc ids; edits
  to doc A don't appear in doc B.

### Phase 2: architect-auth integration

- Add `architect-auth = { path = "../architect-auth/crates/architect-auth" }`
  to workspace deps.
- Mount auth vox services (`CreateEmailPasswordUser`,
  `SignInEmailPassword`, `CurrentSession`, etc.) on the existing
  `/vox` route.
- Server has its own SQLite for auth state (separate from CRDT
  persistence).
- Client `ServerRegistry` stores per-server session tokens.
- **Test**: create user, sign in, get a session token, use it in
  subsequent vox calls. (Capability layer not yet enforcing.)

### Phase 3: Capability middleware

- Define `CapabilityToken` + Ed25519 signing.
- `ServerMiddleware` parses `?cap=<token>` from the WS URL,
  attaches `Capability` to request context.
- Every dispatcher's `subscribe` and `apply_update` checks
  capability against the requested `doc_id`.
- **Test**: anonymous client with a valid share token can
  subscribe to the doc it's scoped to, can't subscribe to other
  docs.

### Phase 4: Project ACL + share-link service

- ACL frontmatter convention on project-doc root page.
- Server-side resolver: wiki-link → auth user id, via a maintained
  basename index over the org vault.
- `ShareService::{create, list, revoke}` vox methods.
- **Test**: project admin creates a share link, anonymous client
  redeems it, gets read-only access. Admin revokes, client's
  next subscribe fails.

### Phase 5: Knowledge as platform

- Migrate the in-progress Knowledge entities to `entity_crdt!`
  (pick up the paused work).
- Wire Knowledge `*Repo` dispatchers on `/vox`.
- Two-tier vault model: `vault/org` + `vault/project/<uuid>`.
- Backlink index maintained server-side.
- Frontmatter index maintained server-side.
- **Test**: create a Person page with wiki-link from a project
  member entry, ACL resolves it to grant access.

### Phase 6: Custom views

- `BasesQuery` parser + executor (port from main; ~1300 lines on
  knowledge-proto).
- A small library of view components: `KindList`, `KindKanban`,
  `KindCalendar`, `KindGallery`. Each is generic over "page set
  with these frontmatter shape expectations."
- **Test**: define a Bases query for `kind: task`, render as
  kanban grouped by `status`. Update a task's status frontmatter
  via the kanban drag-drop; observe the change in another tab.

### Phase 6.5: Properties

Inserted 2026-05-14 after a research pass on Obsidian's Properties
feature, the TaskNotes plugin (`~/Development/research/tasknotes`),
and our own production vault `~/Documents/The Observatory`. The
existing `frontmatter_json: String` (top-level, untyped) blocks
faithful TaskNotes-style modelling — specifically nested structs in
lists (`blockedBy: [{uid, reltype}]`), typed dates for Bases
comparisons, status-with-metadata, and concurrent-safe ordering.

Two sub-phases:

**6.5a — server-side schema core**:

- `knowledge-proto::property_schema` module: `PropertyType`,
  `PropertyDef`, `KindSchema`, `PropertySchemaRegistry`,
  `FieldRenames`.
- Property types supported in 6.5a: `Text`, `Multitext`, `Number`,
  `Checkbox`, `Date`, `Datetime`, `Tags`, `Aliases`, `Link`,
  `LinkList`, `EnumWithMetadata` (status/priority with `color`,
  `icon`, `order`, `isCompleted`, `autoArchive` flags),
  `Struct(named-fields)`, `Computed(read_fn)`, `LexoRank`, `Json`.
  `Recurrence(RRULE)` + `Duration(ISO 8601)` defer to 6.6.
- Hybrid storage: hardcoded built-in schemas for `task`, `project`,
  `area`, `person`, `daily`; user-extensible via `kind: schema`
  Pages in `vault://org` that the indexer merges at boot.
- Best-effort coerce + warn on type mismatch — store original in
  a shadow field, surface a warning. Matches Obsidian's behavior.
- Phase 5b `FrontmatterIndex` upgrade: schema-aware decomposition
  so `List<Wikilink>` indexes per element and `Struct` indexes by
  `<parent>.<child>` paths. Bases executor uses the schema to
  type-coerce comparisons.
- `LexoRank` primitive (~80 lines, helper crate or inline module).
- **Test**: load 10 representative pages from
  `~/Documents/The Observatory/TaskNotes/` through the schema and
  verify they round-trip without data loss; verify Bases date
  comparison gets typed values.

**6.5b — properties UI + kanban DnD**:

- Properties pane in `knowledge-ui` with type-specific editors
  (text input, chip list, date picker, status dropdown with color
  chips). Drag-to-reorder properties.
- Port HTML5 DnD from TaskNotes' `KanbanView.ts:1284-3372` into
  our `KindKanban`. Drop their 10-second `suppressRenderUntil`
  workaround — Loro has no file-watcher race. Use `LexoRank` for
  inter-card ordering.
- **Test**: playwright spec replaces the button-driven move with
  real `dragTo` between two tabs; verify the `status` frontmatter
  + `sortOrder` LexoRank land correctly on both sides.

### Phase 7: Attachments

- `AttachmentService` with `initiate_upload` + `get_download_url`.
- v0: local filesystem backend with axum-served signed URLs.
- A `kind: attachment` block convention with content_hash +
  filename + mime_type.
- **Test**: upload a file from one tab, download from another tab.
  Verify share-link with `AttachmentsOnly` scope can't subscribe
  to the project doc.

### Phase 8: Client federation UI

- `ServerRegistry` with add/remove server flow.
- Per-server identity in the sidebar ("signed in as
  cody@personal-server").
- Unified views: a "Tasks" view fans out queries to every
  connected server.
- **Test**: connect to two local servers, see tasks from both in
  one view.

### Phase 9: Markdown export/import

- `task export --project <id> --format obsidian --out <dir>`.
- `task import --path <dir>`.
- Round-trip stability test: export → import → export → assert
  bytes equal.

### Phase 10: Per-entity-kind sync (optimization)

- Within a doc, split the broadcast by entity kind.
- Subscribers specify the kinds they care about.
- **Test**: a client subscribed only to `kind: task` doesn't
  receive bytes when `kind: recording_session` blocks change.

## 14. Decisions (formerly open questions)

These were originally open; resolved in a design pass and locked
in here so subsequent code doesn't re-litigate them.

### 14.1 Object store: pluggable, Nextcloud-first

`ObjectStore` is a Rust trait with multiple backends:

```rust
trait ObjectStore {
    async fn initiate_upload(&self, ...) -> UploadTicket;
    async fn get_download_url(&self, id, valid_for) -> DownloadUrl;
    async fn delete(&self, id) -> Result<()>;
}
```

V0 backends:

- **`FsObjectStore`** — `${DATA_DIR}/attachments/<project>/<hash>`. Axum
  serves signed URLs valid for 5 minutes. For dev + tiny deploys.
- **`NextcloudWebDavObjectStore`** — uploads via WebDAV, downloads via
  Nextcloud's public-link sharing (OCS API). Tracks Nextcloud
  share-id alongside the Loro entity. **This is the recommended
  production backend.** It means Task focuses on Knowledge +
  collab CRDT; Nextcloud handles file storage + public sharing +
  expiration + password-protected links + everything Nextcloud
  already does well.
- **`S3CompatObjectStore`** — for users who already have MinIO /
  Backblaze / AWS. Phase 7 v1.

The wider implication: **Nextcloud is a first-class peer**, not
just a file backend. A Task server may be configured to:

- Use Nextcloud OIDC as its primary auth provider (instead of
  architect-auth-managed accounts).
- Sync Knowledge `kind: task` blocks to Nextcloud Tasks (CalDAV).
- Sync Knowledge `kind: event` pages to Nextcloud Calendar.
- Store attachments via Nextcloud WebDAV.
- Generate public share links via Nextcloud OCS (the anonymous
  share-link primitive in §5 becomes a Nextcloud share token,
  which works in any client, not just ours).

A Task server with no Nextcloud configured falls back to
filesystem-backed everything (FsObjectStore + architect-auth
local accounts). Pluggability all the way down.

### 14.2 Server ID — derived from public key

Ed25519 keypair generated at first boot, persisted to
`${DATA_DIR}/server.key`. Server ID is the public key's hex
encoding (or its SHA-256 hash, truncated for friendliness).
Clients verify capability tokens against this key. Rotating the
keypair = "new server" identity from clients' POV — they re-pair.

### 14.3 Auth as its own microservice

Architect-auth runs as a separate process with its own database
(`${DATA_DIR}/auth.db`). Task-server holds an `AuthClient` that
talks to it over vox RPC. Architect-auth already supports `vox`
as a feature. Sessions are cached on Task-server's side so the
hot path is one in-memory check, not a network round-trip.

For dev / small deployments, both processes run on the same
machine — communication over `localhost`. For scale, separate
hosts. **The architecture pretends they're separate from day one
to make the scale-out boring.**

### 14.4 ACL conflict resolution

Loro's default last-write-wins per-peer is fine for v0. The
edge case (admin A grants, admin B revokes concurrently) is
documented but not addressed in v0. Admins should coordinate.
Add tombstones in v2 if it becomes a real problem.

### 14.5 ACL frontmatter shape — YAML

```yaml
acl:
  # Symbolic groups that resolve at runtime.
  read: ["@org-members", "[[Cody]]"]
  write: ["[[Cody]]", "[[Alice]]"]
  admin: ["[[Cody]]"]

  # Active share tokens (server maintains the source of truth in
  # the project doc's `share_links` root container, but the
  # frontmatter can mirror the active ones for visibility).
  share_links:
    - id: 4f3a-...
      scope: read
      created_by: "[[Cody]]"
      expires: 2026-06-01T00:00:00Z
      label: "Acme review"
```

Symbolic groups: `@org-members`, `@org-admins`, `@everyone`
(authenticated, any org), `@anyone` (truly public — accepts
anonymous tokens too).

### 14.6 Anonymous edit tracking + claim flow

Each anonymous share token gets a stable Loro peer id of the
form `share-link-<token-uuid>`. All edits under that token bear
that peer in Loro's history.

When the anonymous user later creates a real account:

```rust
#[vox::service]
trait AnonymousClaim {
    /// User has signed in (architect-auth session). They had
    /// prior edits via share link `token_id`. Claim them.
    async fn claim_anonymous_session(
        &self,
        token_id: Uuid,
    ) -> Result<ClaimSummary, ClaimError>;
}
```

The server records `(peer_id = share-link-<token_id>, user_id,
claimed_at)` in an `anonymous_claims` table. The display layer
uses this table to substitute the friendly user name for the
share-link peer id when rendering history.

Loro's history is immutable — we don't rewrite the peer in the
doc. The claim is metadata applied at render time. Per-server:
claiming on Server A doesn't claim on Server B (different
tokens, different histories).

Edge case: a single share link might be used by multiple
humans (URL was forwarded). First-claim-wins; subsequent
claimers are told the session is already attributed. Admin can
manually reassign via a privileged endpoint if needed.

### 14.7 Cross-server wiki links — hybrid

Three syntaxes, in order of intent:

| Syntax | Resolves to | Scope |
|---|---|---|
| `[[Acme]]` | Page in **current server's** vaults | Local |
| `[[@work/Acme]]` | Page on server with local alias `work` | Cross-server, alias-mapped |
| `[[https://server.example/page/<uuid>]]` | Page by absolute URL | Cross-server, max portable |

**Key design rules:**

1. **Resolution is 100% client-side.** Servers never tell each
   other about cross-server links. No federation protocol, no
   leaked data.
2. **Aliases live in the client's `ServerRegistry`.** When you add
   a server, you choose a short alias for it (`work`, `studio`,
   etc.). The alias is local to your client install — different
   collaborators may have different aliases for the same server.
3. **Backlinks are server-local.** A page on server A does NOT
   see "incoming wiki links from server B." Backlinks are an
   internal-to-this-server index.
4. **Export to markdown** preserves syntax as-is. Bare local
   names are local; `@alias/` is recipient-mapped; absolute URLs
   work for anyone.
5. **Broken cross-server links** render like Obsidian's
   "non-existent page" indicator — clickable, with an option to
   create on the target server if you have permission.

The `LinkRef` proto type extends to carry `server_alias:
Option<String>` populated when the source uses `@alias/`. The
block's `refs_json` stores this so the renderer can dispatch:

```rust
match link.server_alias.as_deref() {
    None => resolve_local(link.target_linkpath, current_vault),
    Some(alias) => {
        let server_url = client_registry.lookup_alias(alias)?;
        resolve_remote(server_url, link.target_linkpath)
    }
}
```

When the target server is offline, the link renders as
"unresolved (alias `@work` offline)" — clickable when the server
comes back online.

### 14.8 Nextcloud interop scope

Beyond the object-store note in §14.1, the Task ↔ Nextcloud
interop matrix:

| Concern | Nextcloud handles | Task handles |
|---|---|---|
| **Files** (media, exports) | Storage, versioning, public links | Reference (path + sha + nextcloud-share-id) |
| **Auth** | OIDC provider (if configured) | Session token, ACL resolution against the OIDC user_id |
| **Calendar events** | CalDAV storage | Mirror `kind: event` pages → VTODO/VEVENT |
| **Tasks (CalDAV)** | VTODO storage | Mirror `kind: task` blocks → VTODO |
| **Email** | SMTP / IMAP, Mail app, address book | Inbox feature (deferred); `kind: email_thread` pages |
| **Public sharing** | OCS public-link API | Generate via OCS; embed link in project page |
| **User directory** | Nextcloud users + groups | `[[Person]]` pages with `nextcloud_user_id` frontmatter |

Each integration is opt-in. A Task server with no Nextcloud
configured is fully functional standalone — it just runs
filesystem-backed and architect-auth-managed.

## 14.9 Communication as Knowledge (chat, email, decisions)

Communication fits the Knowledge model **if granularity and
storage location are right**:

- **One thread = one Page = one CrdtDoc.** Threads live as
  `comms/thread/<uuid>` sub-docs under the centralized
  `vault://comms` (see §4). Per-thread docs scale: busy orgs
  have 50k+ threads over time, so one mega-doc would be wrong;
  per-thread docs sync only what users are looking at.
- **Each message is one Block** on the thread page. Frontmatter
  per-block: `sent_at`, `from: [[Person]]`, optional
  `in_reply_to: <block-id>`.
- **Block refs are message refs.** A project page carries
  `derived_from: [[Email: Acme renewal#^msg-abc]]` to a specific
  message in the centralized comms vault.

Volume is fine: Loro handles 10k+ blocks per thread; if a chat
channel exceeds that, archive into a follow-up thread. The
whole conversation renders as one timeline page.

**Centralized, not per-project.** §4 covers why: comms arrives
before project assignment is known, threads cross projects, and
mail/chat platforms already store this way. Projects link in via
backlinks; the thread stays in `vault://comms`.

### Synthesis via agents (the bridge)

The **agents feature** (deleted with main but worth resurrecting)
is the bridge between raw conversations and structured project
state:

```
agent runtime ── subscribes ──► chat_thread pages
        │                      (vox WorkspaceSync,
        │                       same as any other client)
        ▼
   LLM synthesis
   per-batch
        │
        ▼
   writes via apply_update:
     - `kind: decision` blocks on the project page
     - `kind: task` blocks with assignee/due
     - wiki-links back to source messages
```

Raw transcript = audit trail. Synthesis = what humans read. The
two live side by side — open a project page, see current state +
recent decisions + open tasks, all sourced from conversation but
distilled. Click any decision → see the original message thread.

Agents are vox clients with privileged auth (architect-auth
service accounts or API keys). They subscribe just like the UI.

### Email + chat ingestion

A separate **Bridge process** monitors IMAP / Slack / Discord /
whatever. Same model as the agent runtime — vox client, talks
to Task server.

```
IMAP/Slack/...  →  Bridge  →  vox apply_update  →  Task server
                              (creates/updates
                               kind: email_thread
                               or kind: chat_thread
                               pages with new blocks)
```

Outgoing email: a `SendEmail` RPC the Bridge picks up and
dispatches via SMTP. Outgoing chat: same shape, vendor-specific.

When the server is configured with a Nextcloud (§14.8), the
Bridge writes to Nextcloud Mail too so the email is reachable
from any standard mail client.

## 14.10 CLI + agent readability — primary design constraint

**Everything users or agents interact with is markdown + YAML
frontmatter.** No proprietary binary formats users have to
parse. No schemas only the Task client understands. An LLM
dropped into a vault with `grep`, `cat`, and `find` can do
useful work without the Task client running.

This is non-negotiable. Implications:

1. **The Loro doc serializes to markdown deterministically.**
   The `obsidian.rs` parser/serializer in `knowledge-proto` (~1100
   lines) already does this. Export = render the doc as markdown
   files. Import = parse markdown into Loro entities. Round-trip
   stable.

2. **Two equally-valid agent access modes.**

   | Mode | Use case | Latency |
   |---|---|---|
   | **vox subscribe** | Interactive agent watching for changes | real-time |
   | **CLI / file export** | Batch agent (nightly summarizer, weekly report) | minutes / on-demand |

   Same data, different access pattern. An agent can be written
   the simple way (parse markdown files) and upgraded to the
   live way (vox subscribe) without rewriting business logic.

3. **No hidden state.** Wiki-link resolution rules, ACL formats,
   frontmatter conventions, `kind:` taxonomies — all documented
   + readable. An agent shouldn't have to reverse-engineer
   anything. Spec lives in markdown in the org vault.

4. **CLI completeness matters.** The `task` binary must expose
   every read path the UI does:

   ```
   task list-pages [--kind <k>] [--vault <v>]
   task get-page <id-or-path> [--format markdown|json]
   task put-page <path>                       # create/update
   task search "<query>"                      # FTS over body
   task graph --from <page>                   # outgoing links
   task backlinks <page>                      # incoming links
   task subscribe --doc <id>                  # tail changes
   task export --vault <v> --out <dir>        # markdown dump
   task import --path <dir>                   # markdown ingest
   ```

   Already partially there with `task new-task`, `task new-project`,
   `task list`, `task set-done`. Round out in Phase 5/6.

5. **Frontmatter conventions are part of the platform contract.**
   Documented `kind:` taxonomy, recognized property names, ACL
   schema — these belong in the spec, not buried in code.

## 15. What this is NOT

To prevent scope creep:

- **NOT** a real-time filesystem sync (Obsidian Sync). Markdown
  round-trip is a button, not a daemon.
- **NOT** a full Notion/Tana clone. We're not building a
  database-as-pages UI surface at first. Pages with frontmatter
  + Bases is the simplest version of that idea.
- **NOT** end-to-end encrypted. Servers see the CRDT bytes. E2EE
  is a future direction but adds significant complexity.
- **NOT** P2P. There's always a server. Two clients sync via the
  server they share, never directly.
- **NOT** an attempt to replace Git for source code. The
  `kind: project` page might contain git URLs and commit refs,
  but the source repository itself stays in Git.

## 16. Decision

If this matches your mental model, implementation starts at
Phase 1. Each phase becomes its own short `plans/<phase>.md`
doc + a feature branch + a PR. The capability layer is the
biggest risk surface; we'll write end-to-end auth tests before
exposing any of it to the network.

If anything in this document is off — especially the actor
model (§2), the ACL approach (§6), or the phase ordering (§13)
— call it out before code lands.
