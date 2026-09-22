# 5. The Files lanes are the store the apps keep their work in

Date: 2026-09-21

## Status

Accepted. Builds on [ADR 0003](0003-task-as-the-shared-backend.md)
decision 3 ("the manifest is under `resources/`; the bytes are in a File
Root") and decision 4 ("the apps are clients, not backends"), which
between them made the Files lanes the place every app's bytes go without
saying how an app would actually put them there.

## Context

ADR 0003 sends a Signal impulse response, a Session take and a Keyflow
chart's attachments to a File Root, and names the v2 Files lanes as the
way in. Reviewed against that job on 2026-09-21, the lanes were designed
for it and could not do it:

- **An upload could only land in a media root.** `complete` refused with
  "not yet implemented: the byte lane" for every other root, even after
  `send_bytes` had delivered and ingested every byte. A part-sent upload
  was ingested with its holes as zeros.
- **Nothing stopped a lost update.** `FilesFault::Stale` existed and was
  never produced. Two machines saving one session file: last writer wins,
  silently — the exact case "sign in on any machine" creates.
- **An app could not make a place for its bytes.** `adopt` takes a path on
  the server's disk. An app has none to give.
- **The lanes did not know who was calling.** Three lanes each minted a
  different per-process placeholder principal. Nine of thirteen checked no
  per-path grant at all; the four that did recognised only explicit
  grants, so a member signed in from Keyflow was refused their own org's
  roots unless someone had granted them each one.
- **No live stream.** The nested v2 `FilesEvent` existed with nothing
  emitting it.
- **No client.** Upload is begin → a vox channel → complete, and a read is
  ticket → a frame stream. Every app would hand-roll both.
- **`ContentRef` could not mean exact bytes.** `{root_id, path}` names
  whatever is at the path now.

## Decision

### 1. Access is the caller's role united with their grants

`files::lane::caller` is the one place a lane learns who is calling: the
principal the gate resolved (`architect::permissions_gate::caller`), or
the process itself for an in-process call with no gate. What that caller
may do at a path is their **org role** — from the membership row the
server injects through `FilesBackend::set_memberships` — united with
their **explicit grants**. Owner, admin and member hold everything on
content; any other role reads; managing roots (adopt, rename, release) is
an owner's or admin's act.

No membership row is no baseline. That keeps the rule
`memberships::role_for` states — "`None` means NOT A MEMBER" — so a client
who signed up to review one deliverable holds that folder and nothing
beside it: not in a listing, a read, a search or the live stream.

Every lane now checks.

### 7. One Files API

The v1 `FilesService` — 37 ungrouped methods, no per-path checks, its own
event stream — is deleted rather than kept beside the lanes. What it could
do that the lanes could not moved into them (`RootsService::browse_area`,
`VersionService::{browse_at, copy_forward, hint_activity, collect}`,
`SyncService::{residency, set_residency, apply_residency}`,
`MediaService::rendition_info`, `CurationService::named_version`,
`ReviewService::{find, reviews}`), and every caller — the web UI, the CLI,
the daemon, sync, WebDAV, the share-link lane — uses them. The share-link
lane checks a link's scope itself and then calls the lanes as the server
(`files::lane::caller::on_behalf_of_link`), because a link holder is not a
person and the lanes hold nothing for one.

### 2. An app asks for a root by name

`RootsService::create { dir, name, flavor }` makes a directory relative
to the org's files area and adopts it. Idempotent on `dir`, so an app
calls it on every start instead of remembering whether it has; a
directory that exists and is not a root is refused, never adopted by
accident. The backend's own bookkeeping names are reserved.

### 3. A save can be safe

`UploadSpec::expect` — `Absent`, or `Content(etag)` — is checked when the
upload lands, under the root's lock. A mismatch lands nothing and fails
`Stale`. The etag `complete` returns is the landed file's own address,
read back from where it sits, because placement decides the address and
the next save compares against it. A software root has no chunk store and
reports `blake3:<hex>` instead.

Bytes now land in any root, and only once every declared byte has
arrived.

### 4. One live stream, filtered per subscriber

`TreeService::events(root: Option<RootId>)` carries every lane's events,
nested by lane, and every legacy event translated — so a v2 subscriber
hears checkpoints the cadence engine took, not only what arrived through
a v2 method. Each subscription is filtered for the caller who opened it.

### 5. The client is a crate

`features/files/files-client` wraps the lanes as an app uses them —
`ensure_root`, `put`/`put_reader` with a `Save` policy, `get`,
`read_range`, `read_to`, `resolve`/`get_ref` for a pinned reference, and
`events`. It holds generated clients and opens no connection, so it runs
over `task_dial` in a browser, `task_client` natively and a `LocalServer`
in a test, and the gate checks it builds for wasm.

### 6. A reference may pin its bytes

`ContentRef` gains `content`: the etag a save reported. A reader resolving
a pinned reference learns whether the path still holds those bytes, and
fetches them by address (`MediaService::read_content`) when it does not.

## Consequences

**An app can keep its work in Task end to end.** `tests/integration/tests/
it/app_store.rs` is the chapter: a member makes a store, saves safely, a
late save from another machine is refused, and a client holding one
granted folder sees none of it. The seed plants the same thing — a root
made through `create`, audio saved through `files_client`, and
`sample:single-master-loop` pinned to it — so a demo user reaches it.

**Access now depends on membership rows.** A server with no home identity
installs no membership source, and then only grants convey — which is how
the integration suite's own people behave. Role lookups are cached for ten
seconds, so a revoked membership stops conveying on the Files lanes within
that window.

**The `/blobs` attachment store stays.** Song stems and note attachments
keep using it; moving them onto File Roots was considered and deferred by
decision, not by oversight.

**Still to do, each its own piece of work:** a device principal for the
sync lane,
upload sessions that survive a restart, an HTTP fallback for a browser to
read an original without the byte lane, and MCP/CLI verbs for putting and
getting bytes.
