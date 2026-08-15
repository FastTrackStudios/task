# Handoff — federation Phase 2 + server-mediated org creation + tier lint

Session date: 2026-05-22 → 2026-05-23
Branch: `main`

## What landed (10 PRs)

### Phase 2 scaffolding

- **#84** — defaults: server data root → `~/.task/`, client vault root
  helper `org_proto::default_client_vault_root()` →
  `~/Documents/Task/`. `TASK_DATA_ROOT` still overrides. Server-side
  helpers (`default_blob_root`, `default_keypair_path`) funneled
  through `DataRoot::from_env`. Dropped four dead
  `default_*_db_path` helpers.
- **#85** — `mount-proto` + `mount` crates: `Mount` entity (UUID PK on
  `project_id`), `BackendKind` enum (Filesystem / Nextcloud /
  VoxProxy), `ContentBackend` trait, `Filesystem` impl, `MountRegistry`
  with `mounts.toml` round-trip. CLI: `task mount add|list|rm|path`.
  `--under-vault` resolves against the client vault root.
- **#86** — `task auth signup` wired into the CLI
  (`architect_auth::create_email_password_user`). First user signed up
  in a fresh org is its de-facto owner.

### Server-mediated org management

- **#87** — `OrgManagementService` RPC mounted at `/server/vox`
  (one per task-server process, not per-org). `create_org` /
  `list_orgs`. Bootstrap mode (`is_bootstrap()`) accepts
  unauthenticated `create_org` when no orgs exist; normal mode
  validates `session_token` against the home org's `auth.sqlite`.
  `AppState::orgs` became `Arc<RwLock<HashMap>>` so `create_org`
  hot-adds the new `OrgAppState` into the live dispatcher — no
  server restart needed. CLI: `task org create [--server <url>]`.
  Old `task org init` kept as a documented local fallback.

### Federation pointers

- **#89** — `Location.same_as: Option<String>` for cross-org
  "same physical place" pointers (JSON name `sameAs`,
  `#[architect(filterable)]`).
- **#91** (this session's last) — `ProjectInfo.same_as`, mirror of
  Location's pattern, for cross-org project collaboration.

### Wiki refactor

- **#88** — moved cookbook from `<vault>/Cookbook/` to
  `<vault>/Wiki/Cookbook/` (cookbook is part of the wiki).
- **#90** — lifted the wiki out of the vault: layout becomes
  `<org>/{vault, wiki/{Knowledge, LLM/{Memories, Journals}},
  attachments}`. `OrgRoot` gains `wiki_dir()`,
  `wiki_knowledge_dir()`, `wiki_llm_dir()`. `WikiLive` rebased to
  open at a wiki root directly (`paths::WIKI_ROOT` = `""`).
  `cookbook::COOKBOOK_DIR` = `"Cookbook"`; scanner takes
  `wiki_root` not `vault_root`. Migrator (`migrate-md-to-cook`)
  takes `<wiki_root>` now.

  Layered access rule (convention; lint enforces):
  - `vault/` ────link→ `wiki/Knowledge`, `wiki/LLM`
  - `wiki/LLM/` ──link→ `wiki/Knowledge`
  - `wiki/Knowledge/` stays self-contained.
- **#91** — `wiki_graph::tier_lint` + `task wiki lint-tiers`
  CLI enforces the above. Resolves `[[Page]]` Obsidian-style
  (same-tier > Knowledge > LLM > Vault). Federated `@org/Page`
  links skipped. Exit 1 on violation for CI/pre-commit use.

## Live state

```
~/.task/                              # default data root
├── server-key.ed25519
├── blobs/
└── orgs/
    ├── codywright/      [home]       cody@codywright.live (owner)
    ├── fasttrackstudios/
    ├── fasttrackaudio/
    ├── tombrooksmusic/
    ├── cbu/
    └── days-to-praise/  (new this session)
```

Each org has:
```
<slug>/
├── org.toml
├── auth.sqlite (+ timer / finance / identity / agent-tasks)
├── attachments/
├── vault/                            # personal: Journal/, Projects/, Operations/
└── wiki/
    ├── Knowledge/                    # curated (codywright has 54 pages incl. Cookbook/*.cook)
    └── LLM/
        ├── Memories/
        └── Journals/
```

Server is reachable at `127.0.0.1:18080`:

- `/.well-known/task-server.json` — discovery doc (6 orgs)
- `/org/<slug>/{health,vox}` — per-org RPC
- `/server/vox` — `OrgManagementService` (bootstrap + create)

### Seeded data (not committed — lives in `~/.task/`)

**Locations** (each org has `vault/Operations/Locations/...`):
- `codywright` — Home → Home Studio (nested via `parent_id`)
- `cbu` — Main Campus → Room 138/147/Control Room 104/101/108
- `tombrooksmusic` — Adams Business Park
- `fasttrackaudio` — Studio with `sameAs: @codywright/home-studio`
- `fasttrackstudios`, `days-to-praise` — (none)

**Projects** (each org has `vault/Projects/<name>/<name>.md`):
- `codywright` — Mealplan, Fitness, Scheduling
- `tombrooksmusic` — PNG Worship Collective Album (canonical)
- `fasttrackaudio` — PNG Worship Collective Album
  (`sameAs: @tombrooksmusic/png-worship-collective-album`, same
  UUID as the canonical), Mixing Practice
- `fasttrackstudios` — Task, Architect
- `days-to-praise` — Build My Life (worship song)

### Auth

Cody is the sole user in every org. Credentials reused across all 6:
- email `cody@codywright.live`, username `codywright`,
  password `taskdev1` (had to bump from `task` — architect-auth min 8 chars)
- active session: home org (`codywright`)

## What's pending

Suggested next moves:

1. **Federation resolver** — `@org/slug` wikilinks (in body text, in
   `same_as` fields) don't yet resolve through the live server. Need
   a resolver that takes `@org/slug` + the discovery doc + per-org
   wiki index and returns either a cached body or a remote fetch.
   This unlocks Phase 3a from `plans/federated-task-platform.md`.
2. **Server-mediated vault/wiki seeding** — bulk seeding has been
   doing `cp` directly into `~/.task/`. The proper federation flow
   pushes files through `/org/<slug>/vox` via `VaultSync::put_file`
   (one RPC per file). A `task vault import <local-dir>` would be
   the natural CLI surface.
3. **Hermes integration** — agents need an LLM client that reads
   `wiki/Knowledge/` for context and writes journals/memories into
   `wiki/LLM/{Journals,Memories}`. The dirs exist + are walked by the
   lint; we need an actual agent loop.
4. **architect-auth signup over vox** — `task auth signup` still
   opens `auth.sqlite` directly (same-machine assumption). Routing
   it through `/org/<slug>/vox` requires architect-auth to expose
   signup on its `AuthService` trait (it doesn't yet).
5. **Mount integration into project surfaces** — `task list` /
   project commands should consult `MountRegistry` and show
   unmounted projects as `[remote]`. Phase 2 finish-line.

## Quick-start for the next session

```bash
cd /home/cody/Development/Task

# Server should already be running; if not:
TASK_SERVER_BIND=127.0.0.1:18080 ./target/debug/task-server &

# Probes
curl -s http://127.0.0.1:18080/.well-known/task-server.json | jq
./target/debug/task auth whoami
./target/debug/task wiki lint-tiers --org-root ~/.task/orgs/codywright

# Add a new org if you want
./target/debug/task org create <slug> --name "<Display>"
```

Federation conventions are documented inline:
- `org_proto::OrgRoot::wiki_dir` — layered-access rule
- `wiki_graph::tier_lint` — the lint that enforces it
- `Location::same_as` / `ProjectInfo::same_as` — federation pointers
- `plans/federated-task-platform.md` — full phase plan
