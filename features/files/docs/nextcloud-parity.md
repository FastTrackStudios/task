# Files: the design, and where we stand against Nextcloud

Goal: full file management tied to projects — our own implementation of
what Nextcloud Files does, not a client for a Nextcloud server. Markdown
edits through our Editor; previews and real playback UIs for every file
type; Collabora (or similar) embedded later, not now.

Charter: [#3 The Ideal File Manager](https://github.com/FastTrackStudios/task/issues/3).

This document has three parts: the **design** (settled), the **parity
matrix** (where we are), and the **phased list** (what to build).

---

## Part 1 — The design

### Structure

**The vault is a File Root.** Its live tree is plain `.md` on disk —
portable, greppable, `cp -r`-able, Obsidian-compatible — and the Files API
is the only write path to it, markdown included. Portability is preserved
not by keeping the vault outside Files, but by the vault root's live tree
being ordinary files.

**Every file has a project home.** A project is a page with `type:
project` in frontmatter. One definition — the Files org tree's hardcoded
`Projects/` + `Albums/` directories go away, because arbitrary nesting
cannot be expressed by two directory names.

**A subproject is a project.** Nesting is a frontmatter parent link,
unlimited depth; a folder is optional and physical. Hierarchy lives in the
markdown (portable, greppable) and the filesystem follows where useful.

**Two project types, hardcoded in Rust:** audio production and video
production. Each carries its facets, cadence profile, ignore set and
hydration policy. More as needed; the enum stays small so the sync client
can reason about a fixed vocabulary.

### Paths and identity

**jj versions one canonical path per file.** A file may appear at
additional paths as *links* — editing at any path edits the thing. Tags
produce views ("every note with this tag"), not folder membership; a note
has exactly one folder.

**Identity is layered.** `FileId` is content-derived (hash of the chunk
manifest) — that is dedup, integrity, and cross-org "same bytes"
detection. Frontmatter UUIDs identify things that survive renames and
edits. Cross-org migration **keeps** UUIDs, re-identifying only on an
actual collision.

### Sync and placement

**Selective sync is per-facet, driven by project type.** A mix engineer
subscribes to `Sessions`/`Stems`/`Mixes`; a video editor to
`Footage`/`Proxies`/`Renders`. Everything unsubscribed is a **dehydrated
pointer stub** — the path always exists, bytes arrive on access. Facets
declare atomicity, so subscribing to a Reaper session implies its media
(a session whose audio streams in on first access will glitch).

**The same facet vocabulary drives placement**: proxies on cheap S3,
sessions on the fast NAS. One knob, two consumers.

### Transport

**All vox.** Structural operations and bytes. Native clients (CLI,
desktop, iOS/Swift, TS) need no HTTP; peer-to-peer rides iroh/QUIC, which
the chunk store already uses.

The browser is the one exception, and it is a *transport adapter, not an
API*: a service worker intercepts `fetch`, honours `Range` itself, and
returns a `Response` whose body is a `ReadableStream` fed from vox. That
gives `<video>` native range-seeking without MediaSource buffering code.
The existing signed-URL rendition route stays as a fallback for cold guest
links until the shim is proven.

### Permissions

**architect's, unchanged.** `Rule { resource, actions }` is the
capability; `RoleEngine` maps roles to rule bundles — capabilities are the
primitive, roles are named shortcuts over them. Files already has permit
tables in `permits.rs`.

Two gaps: per-root/per-file checks need direct `engine.check` calls in the
backend (no precedent exists in the tree), and the guest lane should move
off `share_guest.rs`'s wrapper services onto `ScopeEngine` +
`Principal::Guest`.

⚠️ **The gate enforces in production.** A new RPC method missing from
`permits.rs` fails closed.

### History

**History replaces trash.** Deletion is a checkpoint; "recently deleted"
is a lens over jj history with one-click restore. No trash storage, no
second GC to maintain — retention becomes GC policy, which already exists.

**The vault gets its own cadence profile.** Loro handles live multiplayer;
`IfMatch`/sha stays as the cheap optimistic-concurrency check that gives
the editor a precise conflict signal; jj checkpoints periodically. Never
per save — that is thousands of ops a day and jj's op-log will feel it.

### Consolidation

The legacy `attachments` blob store folds into Files. `media`/`MediaGrant`
is not a storage system — it is an auth + streaming lane, and it survives,
repointed.

---

## Part 2 — Parity matrix

Legend: ✅ have · 🟡 partial · ❌ missing · 🚫 deliberately not doing

### Browsing and manipulation

| Capability | | Notes |
|---|---|---|
| List/grid, sort, breadcrumbs | ✅ | explorer shell |
| Inspector sidebar / details | ✅ | preview, metadata, versions, divergence |
| Quick preview overlay | ✅ | Space / double-click |
| Create folder, rename, move, copy, delete | ❌ | **no write RPCs at all** |
| Upload (drag-drop, folder) | ❌ | only WebDAV or share-link file-request |
| Chunked / resumable upload | ❌ | Nextcloud: MKCOL → PUT parts → MOVE assemble |
| Upload conflict handling | ❌ | keep-both / replace / keep-existing |
| Bulk select, ZIP download | ❌ | |
| Favourites | ❌ | |
| Trash / restore | 🟡 | jj has the history; no lens or affordance |
| File drop / request inbox | ✅ | per-token incoming area + `promote_incoming` |

### Versioning

| | | |
|---|---|---|
| Version history | ✅ | **exceeds Nextcloud** — jj, not `.v<timestamp>` files |
| Named versions | ✅ | |
| Project versions, lineage, restart | ✅ | no Nextcloud equivalent |
| Divergence detection + resolution | ✅ | |
| Retention / expiry | ✅ | GC with vault-referenced protect set |
| Auto-snapshot cadence | ✅ | quiescence/debounce + save points |

### Sharing

| | | |
|---|---|---|
| Public links | ✅ | password, expiry, disable, org kill switch |
| Link capabilities | ✅ | comment / download / file_request |
| Download-blocked serves proxy only | ✅ | |
| Access log / download receipts | ✅ | |
| Guest lane | ✅ | anonymous vox scoped to a review |
| **Internal user/group sharing** | ❌ | link-only today |
| Reshare rules | 🚫 | Nextcloud's `SHARE` bit is unpredictable |
| Federated sharing | 🚫 | Phase 3, see charter #22 |
| Share by email | ❌ | |

### Previews and viewers

| | | |
|---|---|---|
| Video preview + filmstrip scrub | ✅ | H.264 proxy 1080/720 |
| Audio rendition | ✅ | peaks are raw PCM, not yet JSON-shaped |
| Review player w/ timestamped comments | ✅ | no Nextcloud equivalent |
| Frame-anchored annotation | ✅ | |
| Version compare | ✅ | |
| **Image thumbnails** | ❌ | Nextcloud has ~25 preview providers |
| PDF preview | ❌ | |
| Office/ODF preview | 🚫 | deferred with Collabora |
| Markdown editing in a root | ❌ | Editor is vault-only |

### Search, tags, metadata

| | | |
|---|---|---|
| Tags (system tags) | ❌ | org tree explicitly defers tag lenses |
| Full-text search of contents | ❌ | |
| Filename/metadata search | ❌ | |
| Saved searches as folders | ❌ | |
| Comments on files | 🟡 | review comments only |
| Activity feed | 🟡 | `FilesEvent` stream + share access log |

### Sync and protocol

| | | |
|---|---|---|
| Desktop sync client | ✅ | `files-daemon`, device identity, selective sync |
| Partial replicas / reconcile | ✅ | |
| Pointer stubs (virtual files) | ✅ | |
| WebDAV | ✅ | current heads, read-write |
| Stable file IDs across move | ✅ | content-derived + UUIDs |
| Directory ETag propagation | ❓ | needed only for third-party clients |
| Push notification of changes | ✅ | vox `#[subscribe]`, vs Nextcloud's notify_push |

### Storage and ops

| | | |
|---|---|---|
| Content-addressed dedup | ✅ | FastCDC + BLAKE3, **exceeds Nextcloud** |
| Object-storage backends | ✅ | storage locations + grants |
| Quota / usage reporting | ✅ | |
| External storage (SMB/FTP/…) | 🚫 | storage locations cover this better |
| Server-side encryption | 🚫 | Nextcloud's own only encrypts contents |
| Workflow engine / retention rules | 🚫 | their least-loved subsystem |
| OCS API shape, `oc:permissions` letters | 🚫 | |

---

## Part 3 — What to build, in order

### Phase 1 — make it a file manager

1. **Write RPCs** — `mkdir`, `rename`, `move`, `copy`, `delete` on
   `FilesService`, transactional and checkpoint-aware so jj wraps each in
   one operation. Add the `permits.rs` entries in the same commit or they
   fail closed in prod.
2. **Upload over vox** — chunked and resumable, plus conflict handling
   (keep-both / replace / keep-existing).
3. **Image thumbnailer** — a browser without thumbnails reads as broken.

### Phase 2 — the project model

4. **Unify "project"** on frontmatter; delete the hardcoded directory
   list. Uniform recursion via parent links.
5. **Project types + facets** — `audio-production`, `video-production`;
   facets carry sync, placement, cadence, ignore, hydration.
6. **Per-facet selective sync** in the daemon, replacing path globs.

### Phase 3 — finding things

7. **Tags**, as views rather than folders.
8. **Search** — names and metadata first, then contents. See charter
   [#17 Searchable - Anywhere - Anything](https://github.com/FastTrackStudios/task/issues/17).
9. **Trash lens** over history, with one-click restore.

### Phase 4 — people

10. **Internal sharing** on architect capabilities; roles as bundles.
11. **Guest lane** onto `ScopeEngine` + `Principal::Guest`.
12. **Per-root/per-file checks** — the first `engine.check` calls in a
    backend.

### Phase 5 — consolidation

13. Fold `attachments` into Files; repoint `media`/`MediaGrant`.
14. Browser service-worker byte lane; retire the fallback route.
15. Markdown editing inside a root (the vault-as-root seam).
16. Activity feed.

---

## Traps

Recorded because they have already cost time on this stack:

- **`include_str!` / CSS `@import` across a repo boundary does not work.**
  A git dep has no stable path on disk, and these are invisible to cargo,
  so they fail at compile time — or, for a `@source` glob, *silently* with
  fewer classes. Export bytes from the owning crate instead.
- **A `[patch]` cannot rename a crate.**
- **New RPC method ⇒ new `permits.rs` row**, or it fails closed in prod.
