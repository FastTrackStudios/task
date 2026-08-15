Decentralized, knowledge-first, offline-first foundation per `plans/decentralized-foundation.md`, all 10 phases.

Branch: `thin-vertical-slice`. Each phase: independently committable, evidence in transcript, ping user at phase boundary.

# Notification protocol

Notify the user (single short message, no preamble) at any of these moments:

1. **Phase boundary** — after the phase's final commit lands. Include: phase #, commit SHA, one-line summary, "next: phase N+1" or "awaiting OK".
2. **Clarification needed** — a design choice not pinned in `decentralized-foundation.md` (e.g., schema field name, choice between two equivalent libs). Pose ONE question with ≤4 options. Don't bundle.
3. **Blocker** — external dep missing, upstream API drift, unresolvable test failure after 3 honest attempts. Surface the diagnostic, propose two paths.
4. **Scope creep risk** — if a phase looks like it will touch code outside the stated layer (e.g., Phase 3 needs a UI change), stop and ask before proceeding.

Don't notify for: passing tests, intermediate commits inside a phase, file moves, finding the right path through grep. Those are normal progress, not events.

Pause after each phase commit. Don't auto-start the next phase. User confirms or redirects.

# Done definition (overall)

All ten phases hold their per-phase done criteria below. Final commit on `thin-vertical-slice` references this goal doc. `git log --oneline` shows ten phase commits (or one merge per phase). `cargo check -p task-ui` + `cargo check -p task-app-web --target wasm32-unknown-unknown` clean at the tip.

---

# Phase 1: Doc-id transport — ✅ DONE

Already landed as commit `8293408`. Evidence:

- `WorkspaceSync::{subscribe, apply_update}` take `doc_id: DocId` (see `features/project/project-proto/src/sync.rs:91-106`).
- Server `DocRegistry` lazy-opens per-doc (`apps/server/src/lib.rs`).
- `apps/server/tests/doc_isolation.rs` — `distinct_docs_dont_cross_streams` + `same_doc_id_reopens_with_state` pass.
- All 6 prior sync/stress tests updated and pass.

Deferred (track but don't gate): LRU eviction (do in Phase 3 alongside capability), `list_docs` RPC (do in Phase 8 alongside federation UI).

---

# Phase 2: architect-auth integration

Done when:

- `architect-auth = { path = "../architect-auth/crates/architect-auth" }` in `Cargo.toml` workspace deps.
- Server mounts auth vox services on existing `/vox` route. Grep proves `CreateEmailPasswordUser`, `SignInEmailPassword`, `CurrentSession` dispatchers wired into the vox acceptor.
- Server boots with its own SQLite for auth state, distinct path from CRDT persistence. Show `apps/server/src/lib.rs` opening both.
- Client `ServerRegistry` struct exists with per-server session-token storage (in-memory for now; persistence is Phase 8). Show file path + struct.
- New integration test `apps/server/tests/auth.rs`: creates user, signs in, calls `CurrentSession` with the returned token, asserts identity round-trips. Passes.
- `cargo check -p task-ui` + wasm check clean.
- Capability layer NOT yet enforcing — token threading is the only gate.
- Commit on `thin-vertical-slice`. Message references `plans/decentralized-foundation.md` §13 Phase 2.

Clarification likely needed: where auth SQLite lives on disk; whether sign-up is exposed publicly or admin-only on this server. Ask before deciding.

---

# Phase 3: Capability middleware

Done when:

- `CapabilityToken` Facet struct + Ed25519 sign/verify helpers in a new `crates/capabilities` or `apps/server/src/capability.rs`. Show file.
- `ServerMiddleware` parses `?cap=<base64-token>` from the WS upgrade URL, attaches `Capability` to vox request context. Show middleware wiring in `apps/server/src/lib.rs`.
- Every `subscribe` and `apply_update` checks `capability.scope` against requested `doc_id`. Mismatch returns `SyncError::Forbidden`.
- Server keypair lives in a config file path (printed once on first boot). Choose path — likely `~/.local/share/task-server/server-key.ed25519` — and ask if uncertain.
- `apps/server/tests/capability.rs`: anonymous client with valid token scoped to `project/A` can subscribe to it, cannot subscribe to `project/B` (`Forbidden`).
- LRU eviction of `DocRegistry` lands here (carried over from Phase 1 deferred): doc count cap, configurable, default 256. Test that exceeding the cap evicts the LRU doc and that re-opening it restores state from persistence.
- Commit. Wasm + native checks clean.

Clarification likely needed: token format on the wire (base64url? hex?). Pick one, document it.

---

# Phase 4: Project ACL + share-link service

Done when:

- ACL frontmatter shape per §14.5 of `decentralized-foundation.md` parsed off the project-doc root page. Show parser.
- Server-side basename index over `vault://org` for wiki-link → user resolution. Index rebuilds on org-vault change. Show the index struct + update hook.
- `ShareService` vox trait with `create(scope, ttl) -> token`, `list(project_id) -> Vec<TokenMeta>`, `revoke(token_id)`. Wire dispatcher.
- Revoked tokens land in a revocation list checked in Phase 3's middleware.
- `apps/server/tests/share_link.rs`: admin creates share link scoped to `project/X` read-only, anonymous client with that token can subscribe, cannot `apply_update`. Admin revokes, next subscribe fails with `Forbidden`.
- Anonymous edit attribution stub: each connection gets a stable `peer_id` (Ed25519 pub key). Edits carry it. Claim flow is NOT in scope here — Phase 8.
- Commit. Checks clean.

Clarification likely needed: ACL conflict resolution edge cases beyond §14.4. Ask if a case arises that isn't in the doc.

---

# Phase 5: Knowledge as platform

Done when:

- `features/knowledge/knowledge-crdt` migrated to `entity_crdt!` macro for: Vault, Folder, Page, Block, KnowledgeTag, Base. Grep proves no hand-written `RepoLoro` impls remain for these.
- Knowledge `*Repo` vox dispatchers mounted on `/vox` (Page, Block, etc.). Show acceptor wiring.
- Three-tier vault model (per `decentralized-foundation.md` §4): `vault://org`, `vault://comms`, `vault://project/<uuid>`. `DocId::project`, `DocId::org_vault`, `DocId::comms_vault` constructors already exist — wire the open/persistence paths.
- Backlink index maintained server-side on Block updates. Show update hook.
- Frontmatter index maintained server-side (kind, status, anything declared in a Bases query later). Show the kv store + update hook.
- Knowledge UI route renders Page → Blocks for the org vault. Live edit syncs between two browser tabs.
- Migration: existing `Task` entity grows a `kind: task` Knowledge Page representation OR stays as a sibling — pick. Document choice in the goal-doc commit message.
- `apps/server/tests/knowledge_e2e.rs`: create Person page, wiki-link from project member entry, ACL resolves the wiki-link to grant access via Phase 4 resolver.
- Reference Obsidian vault `~/Documents/The Observatory` for shape expectations (frontmatter conventions, link styles). Mention which conventions were borrowed.
- Commit. Checks clean. Browser playwright spec for one Knowledge route passes via `just test-browser-fresh`.

Clarification likely needed: Task ↔ Knowledge Page representation — sibling vs. unified. This is a fork in the road; ask before committing direction.

---

# Phase 6: Custom views (Bases)

Done when:

- `BasesQuery` parser + executor on `knowledge-proto`. Port from main (~1300 lines per `decentralized-foundation.md`). Show LOC and module path.
- View component library on `knowledge-ui`: `KindList`, `KindKanban`, `KindCalendar`, `KindGallery`. Each is generic over a page-set. Wire all four. architect-ui primitives only, theme tokens only — `cargo clippy -p knowledge-ui` clean.
- A demo Base in the org vault: `kind: task` rendered as kanban grouped by `status` frontmatter.
- Browser spec: drag a card across columns, observe frontmatter update in a second tab.
- Commit. Checks clean.

Clarification likely needed: query language surface (YAML-in-frontmatter vs. inline code block). Ask if not already in §4 of the design doc.

---

# Phase 6.5: Properties

Inserted after Phase 6 commit landed. Driven by a research pass on Obsidian core, the TaskNotes plugin, and our production vault. Detailed scope in `plans/decentralized-foundation.md` §13 Phase 6.5.

Three decisions locked with the user 2026-05-14:
- Storage: **hybrid** — hardcoded core schemas in the server binary, page-overridable via `kind: schema` Pages in `vault://org` that the indexer merges at boot.
- Strictness: **best-effort coerce + warn** — match Obsidian; store original in shadow field if coercion fails.
- Status model: **full StatusConfig** — `status` is an `EnumWithMetadata` carrying `{value, label, color, icon, order, isCompleted, autoArchive}`.

## 6.5a — server schema core (no UI)

Done when:

- `knowledge-proto::property_schema` module exists with `PropertyType` enum (Text, Multitext, Number, Checkbox, Date, Datetime, Tags, Aliases, Link, LinkList, EnumWithMetadata, Struct, Computed, LexoRank, Json), `PropertyDef`, `KindSchema`, `PropertySchemaRegistry`, `FieldRenames`. Show file + types.
- Built-in schemas for `task`, `project`, `area`, `person`, `daily`. Modelled on TaskNotes (`~/Development/research/tasknotes/src/types.ts:445-711`) and The Observatory's actual top-key frequency.
- `PropertySchemaRegistry` exposes a merge path: hardcoded built-ins + any `kind: schema` Pages in the org vault, merged at boot.
- `LexoRank` primitive lands (small helper module ≈80 lines).
- `apps/server/src/knowledge_index.rs` `FrontmatterIndex` upgraded to schema-aware decomposition: `List<Wikilink>`, `Tags`, `Aliases` index per element; `Struct` indexes `<parent>.<child>` paths. Show diff.
- `apps/server/src/property_schema.rs` (or equivalent) wires the registry into AppState.
- Bases executor (`knowledge-proto::bases::execute_view`) takes an optional `&PropertySchemaRegistry` and coerces `Cmp` operands by declared type when one is present.
- Test corpus: load ≥5 representative pages from `~/Documents/The Observatory` (any mix — TaskNotes folder, a daily note, an area page) through the schema; assert round-trip preserves every field. Test added to `apps/server/tests/`.
- Coerce-or-shadow: when a value can't coerce to its declared type, the original is preserved in a sibling `__shadow__<field>` JSON entry + a warning logged. Tested.
- `cargo test -p knowledge-proto` + `cargo test -p task-server --lib` clean; UI + wasm checks clean.
- Commit on `thin-vertical-slice` referencing `plans/decentralized-foundation.md` §13 Phase 6.5.

Clarification likely needed: how `kind: schema` Pages serialize their schema content (JSON in frontmatter vs YAML body) — ask if not obvious.

## 6.5b — properties UI + kanban DnD

Done when:

- Properties pane component in `knowledge-ui` rendering type-specific editors for every `PropertyType`. architect-ui primitives + theme tokens only. Wired into the `KnowledgeLive` route as the top-of-page pane.
- HTML5 DnD ported from TaskNotes `KanbanView.ts:1284-3372` into our `KindKanban`. Drop their `suppressRenderUntil` workaround.
- `sortOrder: LexoRank` field added to the `task` kind schema; the kanban writes it on drop.
- Playwright spec uses `dragTo` to move a kanban card across columns; verify both `status` and `sortOrder` on the target page in tab B.
- All native + browser tests pass; UI + wasm clean.

# Phase 7: Attachments

Done when:

- `AttachmentService` vox trait with `initiate_upload(doc_id, filename, mime, size) -> UploadTicket`, `get_download_url(content_hash) -> SignedUrl`. Wire dispatcher.
- v0 backend: local FS at `~/.local/share/task-server/blobs/<content_hash[0..2]>/<content_hash>`. axum route serves signed URLs.
- `kind: attachment` block convention written into a Knowledge Page on upload completion. Content hash + filename + mime in frontmatter.
- Object-store trait abstraction in place so Nextcloud + S3 backends can land later (don't implement them now).
- `apps/server/tests/attachments.rs`: upload via one client, download via another. Share-link with `AttachmentsOnly` scope can fetch the blob but cannot subscribe to the project doc.
- Commit. Checks clean.

Clarification likely needed: max upload size cap; whether to chunk. Ask before picking.

---

# Phase 8: Client federation UI

Done when:

- `ServerRegistry` persisted client-side (location: web → IndexedDB, native → `~/.local/share/task-architect/servers.json`; pick early and document).
- Add-server flow: user enters URL, signs in via Phase 2 flow, session token stored per-server. Per-server identity shown in sidebar ("signed in as cody@personal-server").
- Unified "Tasks" view fans out a query to every connected server, dedupes by `(server_id, task_id)`. Show the fan-out helper.
- `list_docs` RPC on `WorkspaceSync` (deferred from Phase 1) lands here. Used by federation UI to enumerate per-server docs.
- Anonymous edit claim flow: a logged-in user can claim historical anonymous edits made under a `peer_id` they control. Show the migration that rewrites attribution.
- Browser spec: connect to two local servers (boot two on different ports in the test), see tasks from both in one view.
- Commit. Checks clean.

Clarification likely needed: cross-server wiki-link UX — alias picker vs. raw URL paste. Ask, then pick.

---

# Phase 9: Markdown export/import

Done when:

- `task export --project <id> --format obsidian --out <dir>` writes a directory matching Obsidian conventions (frontmatter YAML, `[[wikilinks]]`, attachments dir). Cross-reference shape against `~/Documents/The Observatory`.
- `task import --path <dir>` ingests an Obsidian vault into a new or existing project doc.
- Round-trip test: export → import into fresh server → export → byte-equal assertion (or content-equal with documented normalization rules — pick one).
- `apps/cli/tests/markdown_roundtrip.rs` passes.
- Commit. Checks clean.

Clarification likely needed: handling of Obsidian-specific syntax we don't support (callouts, dataview blocks). Either stub through verbatim or strip — ask.

---

# Phase 10: Per-entity-kind sync (optimization)

Done when:

- Within a single doc, broadcast is split by entity kind. Show the per-kind `broadcast::Sender` map.
- `WorkspaceSync::subscribe` grows a `kinds: Option<Vec<String>>` filter param. `None` = all kinds (back-compat).
- `apps/server/tests/per_kind_sync.rs`: client subscribed only to `kind: task` doesn't receive bytes when `kind: recording_session` blocks change. Measure with a counter on the receiver.
- Benchmark in the commit message: before/after byte-volume for a representative workload.
- Commit. Checks clean.

Clarification likely needed: filter semantics when a wiki-link crosses kinds (does a `task → person` link emit on the task channel, person channel, both?). Ask.

---

# Cross-cutting constraints (all phases)

- `cargo check -p task-ui` + `cargo check -p task-app-web --target wasm32-unknown-unknown` must be clean at every phase commit.
- No `--no-verify`, no skipping hooks.
- architect-ui primitives only in UI; theme tokens never hex; dark-mode default; dumb components. (Per `AGENTS.md`.)
- New entities use `entity_crdt!` macro, not hand-written `RepoLoro`.
- Fakers live in `mod fake { ... }` within the entity file under `#[cfg(feature = "fake")]`.
- Use `use Facet` not `::facet::Facet`.
- Commit messages reference the phase + `plans/decentralized-foundation.md`.
- One commit per phase preferred; multiple commits inside a phase OK if logically separable, but the phase boundary is the notification trigger.
- After every phase commit: pause, notify, wait for user confirmation. Don't auto-chain phases.

# Stop conditions

- After 60 working turns without finishing a phase: stop, surface the blocker. Don't grind.
- Two consecutive test failures with the same root cause that aren't yielding: stop, ask.
- Any phase that wants to spawn a subagent for >30 min of background work: ask first, don't fire-and-forget.
