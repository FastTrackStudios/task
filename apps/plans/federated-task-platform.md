# Federated Task Platform

**Status:** in progress — needs triage (2026-07-27). Multi-phase; per-phase state is not determinable from the tree alone. Phase 3 (one account across servers) is the open head.

Restructure Task from a single-machine PKM with one vault into a **federated multi-org platform** where:

- Every user account has a **personal org** that serves as their identity home.
- A `task-server` instance hosts **N orgs** (a company's main org plus the personal orgs of its members), and orgs are **portable** — `rsync` the directory to a different machine and point a server at it.
- **Project content lives anywhere** on disk or in any backend (filesystem, Nextcloud, S3, …) — the org owns *pointers*, not bytes.
- Different machines sync **only the slices they care about**: a phone might have just one project mounted; a workstation might have all of them; a server might have none of them locally and proxy through Nextcloud.

The point: no part of the data is monolithic. Identity, metadata, and content are three independent axes that can each live on a different machine.

---

## Vision

Today: one user, one `~/.local/share/task-server/auth.sqlite`, one vault root (env var), one machine.

After:

```
                       ┌──────────────────────┐
                       │  task.codywright.live │
                       │  (home / personal)    │
                       │  - codywright/        │
                       │  - identity links     │
                       │    fts → fts.task...  │
                       │    cbu → cbu.task...  │
                       └─────────┬────────────┘
                                 │ federation
              ┌──────────────────┼────────────────────┐
              ▼                  ▼                    ▼
  ┌──────────────────┐ ┌────────────────────┐ ┌────────────────┐
  │ fts.task.live    │ │ tom-brooks-music  │ │ cbu.task.live  │
  │ - fasttrack...   │ │ - tombrooksmusic  │ │ - cbu          │
  │ - fasttrackaudio │ │   (his personal,  │ │   (university) │
  │   (each hosts    │ │    hosted here)   │ │                │
  │    personal orgs │ │                   │ │                │
  │    of members)   │ │                   │ │                │
  └────────┬─────────┘ └─────────┬─────────┘ └────────┬───────┘
           │                     │                    │
           └─────── projects ────┴────────────────────┘
                   (filesystem, Nextcloud, external drives;
                    location is per-machine, content addr is uuid)
```

Cody signs into the home server once. The CLI knows about every linked org and can switch context without re-authenticating. Federated orgs trust the home for identity but maintain their own member list keyed off `(home_url, home_user_id)`.

---

## Topology

### Three layers, three locations

| Layer | What it is | Where it lives | Scope |
|---|---|---|---|
| **Identity** | Cross-org account links, aliases, encrypted remote tokens | Home org's `identity.sqlite` | Per home (one per user) |
| **Org metadata** | Auth users, members, project index, timer/finance DBs, vault frontmatter cache | Per-org dir on whatever server hosts it | Per org |
| **Project content** | Markdown, attachments, media | Anywhere — per-machine mount registry resolves `project_id → path/backend` | Per machine |

A node (server or client) can hold any subset of these. A pure CLI on a phone might hold only the identity layer pointer (cached token). A workstation holds identity + a few mounted projects. A self-hosted server holds an org's full metadata + some projects.

### One server, many orgs

A `task-server` process scans `<data_root>/orgs/` at boot and serves every org it finds. Routing is URL-based: `/org/<slug>/vox`, `/org/<slug>/health`, `/org/<slug>/blobs/...`. Each org has its own auth instance, its own timer/finance DBs, its own member list.

### Discovery

`<server>/.well-known/task-server.json` lists the orgs the server hosts:

```json
{
  "version": 1,
  "orgs": [
    { "slug": "fasttrackstudios", "display_name": "FastTrackStudios", "vox": "/org/fasttrackstudios/vox" },
    { "slug": "fasttrackaudio",   "display_name": "FastTrackAudio",  "vox": "/org/fasttrackaudio/vox" }
  ]
}
```

So `task auth link fts.task.live` is enough; the CLI pulls discovery and asks which org you want.

---

## On-disk layout

### Per-server data root

```
<data_root>/                          # default: $XDG_DATA_HOME/task-server, configurable via TASK_DATA_ROOT
├── server-key.ed25519                # blob signing keypair (cross-org)
├── orgs/
│   ├── codywright/                   # personal org — same shape as any other, plus is_home=true
│   │   ├── org.toml                  # slug, display name, is_home, federation_url
│   │   ├── auth.sqlite               # this org's user/member rows
│   │   ├── identity.sqlite           # ONLY in home orgs: aliases + encrypted remote tokens
│   │   ├── timer.sqlite
│   │   ├── finance.sqlite
│   │   ├── projects.toml             # project index (uuid → slug, title, federation pointer)
│   │   └── attachments/              # local-only attachment blobs (signed via server-key)
│   ├── fasttrackstudios/
│   │   ├── org.toml                  # is_home=false
│   │   ├── auth.sqlite
│   │   ├── timer.sqlite
│   │   ├── finance.sqlite
│   │   ├── projects.toml
│   │   └── attachments/
│   ├── fasttrackaudio/
│   │   └── ...
│   └── tombrooksmusic/
│       └── ...
```

### `org.toml`

```toml
slug         = "codywright"
display_name = "Cody Wright (personal)"
is_home      = true
federation_url = "https://task.codywright.live/org/codywright"
created_at   = "2026-05-22T15:00:00Z"
```

### `projects.toml` (per-org)

```toml
[project.7f3a2b1e-…]
slug      = "website-redesign"
title     = "Website Redesign"
created   = 2026-04-12
home_url  = "https://task.codywright.live/org/codywright/p/7f3a..."

[project.9a2c4d…]
slug = "tom-brooks-album"
title = "Tom Brooks Album"
home_url = "https://tom-brooks.task.live/org/tombrooksmusic/p/9a2c..."
```

### Per-machine mount registry

```
~/.config/task/mounts.toml
```

```toml
# Filesystem mount — vault content lives at a literal path.
[mount.7f3a2b1e-…]
backend = "filesystem"
path    = "/home/cody/Projects/website-redesign"

# External drive — same backend, different path.
[mount.9a2c4d…]
backend = "filesystem"
path    = "/mnt/raid/audio/tom-brooks-album"

# Nextcloud — backed by a WebDAV remote, not a local path.
[mount.4ee1ab…]
backend = "nextcloud"
remote  = "https://cloud.codywright.live"
user    = "cody"
path    = "Documents/Work/client-x"

# Future: S3, Git, etc.
```

Same `project_id` resolves to different backends on different machines. Adding a new machine = `task mount add 7f3a2b1e-… /path/on/this/machine`. No metadata copy needed.

### CLI session file

```
$XDG_DATA_HOME/task/session.json
```

```json
{
  "home": "codywright",
  "active": "fasttrackstudios",
  "servers": {
    "codywright":      { "url": "https://task.codywright.live", "user_id": "...", "token": "..." },
    "fasttrackstudios":{ "url": "https://fts.task.live",        "user_id": "...", "token": "..." }
  }
}
```

`active` is the current org context; `home` is the identity anchor (CLI loads `identity.sqlite` via this token to refresh other org tokens).

---

## Flows

### Signup

1. User signs up on `fts.task.live` (joining the FastTrackStudios company).
2. Server creates **two** rows in `fts.task.live/orgs/`:
   - `<slug>/auth_users`: a row in the `fasttrackstudios` org (their company member).
   - A new personal org dir `cody-fts/` with the user as sole admin (this is their *interim home*).
3. User now has an account they can sign into. By default the interim personal org IS their home.
4. Later, they may run `task org export cody-fts` → `rsync` to their own VPS → `task org claim https://task.codywright.live/codywright`. The fasttrackstudios server replaces the local user row with a federated-member row pointing at the new home URL. The interim personal org dir is archived/deleted.

### Login

1. `task auth login --server <url>` — signs into a specific org (any org, but typically your home).
2. Server returns `AuthSessionBundle` (existing architect-auth flow).
3. CLI writes `session.json` with this server as `home` (first login wins, or `--as-home` to override).
4. If the signed-into org has `identity.sqlite`, the CLI fetches the list of linked orgs and refreshes their tokens in `session.json`. No re-auth needed.

### Linking an org

1. `task auth link <server-url>` — discovery → list of orgs hosted there.
2. User picks one (or `--org <slug>` to skip the picker).
3. Sign in with credentials valid on that remote (one-time).
4. CLI stores `(remote_url, remote_user_id, encrypted_token)` in the **home** org's `identity.sqlite`, keyed by `(home_user_id, org_slug)`.
5. Tokens encrypted at rest with a key derived from the home login (passphrase-derived KDF; details in *Open decisions* §3).

### Switching context

1. `task auth use <slug>` — looks up the slug in `session.json.servers`, sets `active`.
2. If the token has expired, CLI fetches a fresh one from home (`identity.sqlite` holds the encrypted remote credential) without prompting.
3. Subsequent commands (`task timer start`, `task list`, …) route to `servers[active].url`.

### Resolving a project's content

```
task open 7f3a2b1e-…
```

1. CLI queries `<active>/projects.toml` for the project's metadata.
2. CLI queries `~/.config/task/mounts.toml` for a backend assignment.
3. If a mount exists: open via the backend's `Read` impl.
4. If not: print `"Project not mounted on this machine. Run: task mount add 7f3a2b1e-… <path>"`.

A project visible in `task list` but unmounted shows as `[remote]` rather than being hidden.

### Pulling content from federation

For a project whose home is a remote server:

1. CLI knows `home_url` from `projects.toml`.
2. `task project pull 7f3a... --to ~/Projects/website-redesign` calls the remote's vault-sync endpoint (existing `vault::Backend` over vox) to populate a local checkout.
3. CLI auto-writes a `mounts.toml` entry pointing at the new local path.

---

## Backend trait

Project content access is abstracted over a backend:

```rust
trait ContentBackend {
    async fn list(&self, prefix: &str) -> Result<Vec<Entry>>;
    async fn read(&self, path: &str) -> Result<Vec<u8>>;
    async fn write(&self, path: &str, bytes: &[u8]) -> Result<()>;
    async fn watch(&self, path: &str) -> impl Stream<Item = ChangeEvent>;
}
```

Initial impls:

- **Filesystem** (`std::fs` + inotify) — what we have today, just wrapped.
- **Nextcloud** (WebDAV — there's a `reqwest_dav` crate, or roll thin client).
- **Vox-proxy** — read from a remote task-server via the existing vault-sync RPC (lets a thin client like a phone work without local content).

Future: S3, Git (read-only), restic-style content-addressed.

---

## Sync model

Two separate sync channels:

| Channel | What flows | Transport | Volume | Realtime? |
|---|---|---|---|---|
| **Metadata sync** | project index, members, frontmatter, timer/finance rows, tags | vox/WebSocket | KB/s | Yes (subscribe to changes) |
| **Content sync** | markdown, attachments, large media | Backend-native (WebDAV, FS watch, S3 events) | MB–GB | Best-effort, eventual |

The point: don't push 50 GB of audio session files through the vox metadata channel. Metadata fits the existing architect-rpc / vault-sync infra; content has its own pipeline per backend.

A project is **fully synced** on a machine when both metadata (in `projects.toml` / `auth.sqlite`) and content (resolvable via `mounts.toml`) are present. Either can lead the other:

- **Metadata-first**: you see a project in `task list` because the org synced it down. You haven't mounted content yet → pull on demand.
- **Content-first**: you `rsync` a project folder to a new machine, then `task mount add` registers it locally; metadata catches up from the org on next sync.

---

## Migration from current state

Where we are now (post PR #62):

- `apps/server/src/lib.rs` uses `default_*_db_path()` resolvers that return `$XDG/task-server/{auth,timer,finance}.sqlite` — single-org assumption.
- `apps/cli/src/main.rs` has `TASK_VAULT_ROOT` env var (one vault) and `session_store::default_auth_db_path()` that mirrors the server.
- `apps/cli/src/session_store.rs::CliSession` has a single `{token, user_id, org_id}`.

### Phase 1 — Org-rooted paths (no federation yet)

- Replace `default_*_db_path()` with `OrgRoot { path, slug }` → `OrgRoot::auth_db()`, `OrgRoot::timer_db()`, etc.
- `task-server` takes `--data-root <path>` (default `$XDG_DATA_HOME/task-server`). At boot, scans `orgs/`, returns one `AppState` per org wired into `/org/<slug>/...` routes.
- CLI `session.json` grows from one `{token, user_id, org_id}` to `{home, active, servers: {<slug>: {url, user_id, token}}}` — back-compat: old shape upgrades to `home="local", active="local", servers.local={…}`.
- New CLI: `task org init <slug> [--home]`, `task org list`.

**Exit criteria:** running one task-server with two local orgs works end-to-end. CLI can `task auth use <slug>` to switch between them.

### Phase 2 — Project mounts (still single-machine)

- Per-org `projects.toml` (replaces the implicit "scan vault for projects" pattern). Project frontmatter UUID → slug mapping cached here, updated on vault write.
- `~/.config/task/mounts.toml` + `task mount add|list|rm`.
- `ContentBackend` trait + `Filesystem` impl. Refactor `vault-obsidian` reads to go through it.
- `task list` honors mounts: unmounted projects show as `[remote]`.

**Exit criteria:** projects can live anywhere on disk; moving one = updating `mounts.toml`, no metadata edits.

### Phase 3 — Federation (the actual fun)

- `org.toml` with `is_home` + `federation_url`.
- `identity.sqlite` schema + encryption (passphrase-derived AEAD key).
- `task auth link`, `task auth use`, token refresh via home.
- `.well-known/task-server.json` discovery.
- Federated member rows in `auth.sqlite` (currently architect-auth assumes all users are local).

**Exit criteria:** sign into home, `task auth use fts` switches to a remote org transparently.

### Phase 4 — Non-FS backends

- Nextcloud `ContentBackend` impl. `task mount add --backend nextcloud …`.
- Vox-proxy backend for thin clients.

---

## Open decisions

### 1. Where identity lives — top-level vs inside home org

Two options:

**(a) Inside the home org dir** — `<data_root>/orgs/codywright/identity.sqlite`. Identity is data the home org owns. Migrating the home = moving identity with it.

**(b) Top-level, separate from any org** — `<data_root>/identity/<user>.sqlite`. Identity is a server-wide concern, decoupled from any org membership.

I lean **(a)**: matches the federation mental model (your home server owns your identity, including the secret of which orgs you're a member of). When the home migrates, identity goes with it as one unit. (b) introduces a fourth top-level concept; we already have plenty.

### 2. Personal org slug — automatic or chosen

When fasttrackstudios admits a new user, what slug does their interim personal org get?

- **`<user-handle>` directly**: `cody`. Risk of slug collisions if two Codys sign up.
- **`<user-handle>-<server-slug>`**: `cody-fts`. Always unique, ugly.
- **`<server-slug>-personal-<sequence>`**: `fts-personal-7`. Stable, opaque, ugly.

I lean **`<user-handle>-<server-slug>`** — interim personal orgs are meant to be migrated out anyway. Once you `task org claim` and move it to your own home server, you re-slug to whatever you like.

### 3. Encryption of remote tokens in `identity.sqlite`

- **Passphrase-derived AEAD** (argon2id → XChaCha20-Poly1305): only the user can decrypt; CLI prompts for passphrase or uses a kernel keyring.
- **Server-key encryption**: server-key.ed25519 can encrypt; means anyone with the server key can read tokens. Worse for shared servers; fine for single-user.
- **No encryption**: tokens visible to anyone who can read the sqlite file.

I lean **passphrase-derived** with optional `secret-service` (libsecret) integration so it's not a prompt every command. Defer to phase 3.

### 4. Federation identity assertion

When fasttrackstudios receives a request from `cody@home=codywright.live`, how does it verify the home actually vouches?

- **Token bearer**: home issues a JWT-like assertion the peer can validate against the home's public key. Pull pubkey via `.well-known/task-server.json`.
- **Mutual TLS**: heavyweight, deferable.
- **OAuth-style code flow**: home redirects to peer for consent; peer caches consent.

I lean **token bearer with home pubkey discovery** — small, additive on top of architect-auth's session tokens. Each peer caches home pubkeys it has seen.

### 5. CLI command shape

Current: `task auth login`, `task auth org list`, `task auth org use <id>`.

Federated proposal:

- `task auth login [--server <url>]` — sign in (server defaults to current home).
- `task auth link <server-url> [--org <slug>]` — link a remote org to your home.
- `task auth use <slug>` — switch active org.
- `task auth orgs` — list every linked org with sync state.
- `task org init <slug> [--home]` — create a new local org under this server.
- `task org export <slug>` / `task org claim <url>` — migration verbs.
- `task mount add <project-id> <path>` / `task mount list` / `task mount rm <project-id>`.

---

## Federated wiki resolution

Wikilinks must resolve across the whole federation without requiring every org to be fully cloned locally. The trick: separate the **wiki index** (small, sync everywhere) from the **page bodies** (big, fetch on demand). Markdown remains the source of truth on disk.

### Two-layer index

- **`wiki-index.json`** — wire format. One per org, published by its server, human-readable, git-diffable. This is what fans out across federation.
- **`wiki-index.sqlite`** — local query cache. Built from the JSON on import. SQLite + FTS5: indexed lookups, alias matching, substring/full-text search for free. Regenerable from the manifest.

Both are derived from the source markdown. Either can be deleted and rebuilt.

### Cache layout (per-user)

```
~/.cache/task/wiki/
├── fasttrackstudios/
│   ├── manifest.json          # last pulled wire format, verifiable
│   ├── index.sqlite           # FTS5 query cache
│   └── pages/
│       └── Onboarding.md      # cached page bodies — plain markdown
├── codywright/
│   └── ...
```

Per-user (not per-org-root): one machine, one cache, regardless of how many data roots it serves. Org-root backups don't need to drag the cache along — it rebuilds from upstream on first use.

### `wiki-index.json` (wire format)

```json
{
  "org_slug": "fasttrackstudios",
  "federation_url": "https://fts.task.live/org/fasttrackstudios",
  "generated_at": "2026-05-22T19:30:00Z",
  "pages": [
    {
      "id": "p-9a2c…",
      "title": "Onboarding",
      "aliases": ["Onboarding Guide", "New Hires"],
      "path": "Wiki/Onboarding.md",
      "project_id": "7f3a…",
      "updated_at": "2026-05-18T10:00:00Z",
      "summary": "First 30 days at FTS. Accounts, tools, contacts."
    }
  ]
}
```

At ~1KB/page that's 1MB per 1000 pages — trivial to sync for every linked org.

### `index.sqlite` schema

```sql
CREATE TABLE pages (
  page_id      TEXT PRIMARY KEY,
  org_slug     TEXT NOT NULL,
  title        TEXT NOT NULL,
  path         TEXT NOT NULL,
  project_id   TEXT,
  summary      TEXT,
  updated_at   TEXT NOT NULL,
  body_cached  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX pages_title_nocase ON pages(title COLLATE NOCASE);

CREATE TABLE aliases (
  page_id  TEXT NOT NULL REFERENCES pages(page_id) ON DELETE CASCADE,
  alias    TEXT NOT NULL
);
CREATE INDEX aliases_lookup ON aliases(alias COLLATE NOCASE);

CREATE VIRTUAL TABLE pages_fts USING fts5(
  title, aliases, summary, body,
  content='pages', content_rowid='rowid'
);
```

### Link syntax

Same-org links stay vanilla — no change:

```
See [[Onboarding]] for the new-hire flow.
```

Cross-org links use a mention-style prefix:

```
See [[@fts/Onboarding]] for the company flow.
Compare to [[@cody/Onboarding]] for personal notes.
```

`@<org-slug>/<page-title>` is unambiguous, parses cleanly as `(org, page)`, and degrades to a "broken link" in vanilla Obsidian (acceptable — canonical viewer is Task). Same-org `[[Onboarding]]` is preserved as the default to keep Obsidian-compat for personal vaults.

### Resolution order

`[[Onboarding]]` (same-org) tries:

1. Active org, local vault (current behavior).
2. Active org's `index.sqlite` → if body uncached, fetch on demand.
3. Other linked orgs *only* if the title is unambiguous. Two matches → render as `[[Onboarding (ambiguous: @fts, @cody)]]`.
4. Miss → render as "create" stub in the active org (Obsidian-style red link).

`[[@fts/Onboarding]]` (cross-org) tries:

1. `wiki/fasttrackstudios/index.sqlite` → fetch body if uncached.
2. Miss → render as "remote org not synced" (suggest `task auth link fts.task.live`).

### Fetch-on-demand

On a cache miss for a known page:

1. Look up `pages` row → get `path` + `home_url`.
2. Pull `home_url/<path>` via the existing `vault::Backend` RPC over vox.
3. Pull referenced assets (`![](...)`, `![[…]]` embeds) alongside.
4. Write to `~/.cache/task/wiki/<org>/pages/<slug>.md`, set `body_cached = 1`, populate `pages_fts.body`.
5. Render.

### Pinning

Pages can be pinned for guaranteed offline availability:

```
task wiki pin @fts/Onboarding
task wiki unpin @cody/Old-Notes
task wiki pinned
```

Pins live in `~/.config/task/pins.toml` and get refreshed on a schedule.

### Federated search

```
task wiki search "onboarding"            # local + cached + pinned
task wiki search "onboarding" --federated # fan out to every linked org's /wiki/search RPC
```

Each linked org runs its own FTS5 against its `wiki-index.sqlite`; results merge in the CLI with org badges so you know where each hit came from.

### Vectors (deferred)

Semantic search is a separate concern, layered on top later: `~/.cache/task/wiki/<org>/vectors.lance` with its own embedding pipeline. Don't conflate it with the resolution index. lancedb is appropriate when we get there; sqlite FTS5 is the right tool for now.

### Phase fit

Federated wiki resolution lands as part of Phase 3 (federation) — it depends on identity/`.well-known` discovery (so we know which orgs to pull indexes from) but is independent of the project mount system (a parallel resolution path).

---

## Non-goals (for this plan)

- **Real-time co-editing across orgs.** That's a CRDT-layer concern, deferred to the loro-text-editor-upgrade.
- **Conflict resolution between machines on FS backends.** Initially: last-write-wins per file. CRDT only at the per-document layer once the editor work lands.
- **End-to-end encryption of org content.** Server hosts can read; trust boundary stops at the org-server. E2EE is its own multi-month design.
- **Multi-tenant resource quotas.** A self-hosted task-server hosting N orgs has no quota system yet — assume cooperative single-user.
- **Web UI.** Everything described here works against vox endpoints; the web client (`task-app-web`) follows once the data model settles.

---

## Risks

- **Token rotation across federations**. If home rotates its session secret, every linked org's cached home-pubkey verification stays valid (pubkey doesn't rotate with session secret), but the *user's* tokens to peer orgs need re-issuing. Plan: auto-refresh via identity.sqlite on 401.
- **`mounts.toml` drift across machines**. Each machine has its own — by design — but users will expect "I added a project on my laptop" to "just appear" on the desktop. Solution: the org's `projects.toml` is synced; the mount registry is local. `task mount auto-discover` can scan likely paths (`~/Projects`, `~/Documents/Task/orgs/<slug>/projects/`) and pre-fill.
- **Nextcloud as a single point of failure**. If the Nextcloud backend is down, content for those projects is unreachable. Plan: backend impls advertise an `availability_state`; CLI gracefully shows `[unavailable: nextcloud cloud.example.com unreachable]`.
- **Identity sqlite size**. Currently architect-auth assumes local-only users. A federated `auth_users` row needs `(home_url, home_user_id)` as the natural key. This is a schema migration in architect-auth, not just downstream.

---

## What stays the same

- **architect-rpc / vox** is unchanged — federation routes through it, not around it.
- **architect-auth** stays as the auth provider, just with the user table extended for federated members.
- **vault-obsidian + vault::Backend** stay — they become *one* `ContentBackend` impl.
- **Loro / CRDT editor** is orthogonal; this plan doesn't touch it.
- **architect-ui** is orthogonal; UI changes (server picker, mount manager) come later.

This is a path forward, not a rewrite. Phase 1 alone is a useful refactor even if phases 2–4 never land.

## Cross-org duplicate guard (2026-07-02)

Nothing prevents the same task uuid appearing in two orgs (seeded
copies, future federation sync). The web store keys rows by task id,
so a true same-id duplicate would shadow its sibling and route
mutations to whichever org loaded last. Before federation ships:
`task doctor` check for cross-org id collisions + a dedupe/merge
story. (The 2026-07-02 "doubled tasks" turned out to be different
uuids — seeded content copies, deleted from codywright — but the
same-id case is still unguarded.)
