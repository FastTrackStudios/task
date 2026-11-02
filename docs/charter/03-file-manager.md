# The Ideal File Manager

_Charter #3 (formerly issue #3)._

Full file management tied to projects — our own implementation of what a cloud storage provider does, over content-addressed storage with real version history.

This charter is the **map**. The testable detail lives in [`features/files/spec/files.md`](../../features/files/spec/files.md) as tracey requirements; the design of record is [`features/files/docs/nextcloud-parity.md`](../../features/files/docs/nextcloud-parity.md).

Cite a requirement by id — `files.scale.large-media` — in issues, commits and code (`t[impl files.scale.large-media]`).

## Hard requirements

### Scale

- **`files.scale.large-media`** — Any size streams both ways; seeking fetches only what's under the playhead.
- **`files.scale.small-files`** — Directory work is proportional to the directory, not its file count; identical content stored once.
- **`files.scale.capacity`** — Capacity grows by attaching a location. No downtime, no migration, no path changes.

### Manipulation

- **`files.write.surface`** — `mkdir` / `rename` / `move` / `copy` / `delete` over the network, transactional, over sets as readily as single files.
- **`files.write.upload`** — Chunked, resumable, and a collision asks rather than guesses.

### Liveness

- **`files.live.propagation`** — Changes render locally before the server answers, and reach every other client without a refresh.

### Placement

- **`files.sync.selective`** — Devices subscribe to a project type's facets; everything else is a stub that hydrates on access.

### Topology

- **`files.topology.multi-server`** — Content spans servers as one namespace; no server is required in the path.
- **`files.topology.federation`** — Files and folders share across server boundaries as first-class items, not download links. Model in [#22](22-self-host-federation-p2p.md).

### Findability

_The query surface is [#17](17-search.md). These are what Files owes it._

- **`files.index.extraction`** — Text, speech and shot content extract themselves as bytes arrive.
- **`files.index.local`** — On your own hardware, with open models. A third party is never a prerequisite.
- **`files.index.regions`** — A result is a time range, a page, a block — not a file.
- **`files.index.portable`** — Indexes are plain files beside the media, readable without this app.

### Access

- **`files.access.internal-sharing`** — A person or team is granted a folder, without minting a public link.
- **`files.access.granularity`** — Checked at the path acted on, not at the root.

### Concurrency

- **`files.concurrency.advisory-lock`** — Unmergeable files are flagged, never locked; work is never blocked.

### Devices

- **`files.device.control`** — Pin for offline, throttle and resume transfers, revoke and wipe a lost device.
- **`files.device.ingest`** — Camera rolls, stills and voice memos arrive without a manual upload step.

### Organisation and accountability

- **`files.organise.manual`** — Tags and favourites the machine would never infer. A tag is a view, not a folder.
- **`files.organise.activity`** — Who did what to which file, when.

### Handoff

- **`files.handoff.editor`** — Results land in the editor as a bin or a timeline, with region bounds intact.

### Version control

- **`files.version.unit`** — History addresses what the application opens — a file, a directory, or a database row.
- **`files.version.keep-both`** — Divergent edits both survive, attributed and openable. Never an overwrite, never a synthesised merge.
- **`files.version.restore`** — A restored version opens, with its media still resolvable through renames.
- **`files.version.native`** — The application's own backups and caches are absorbed, never shown as versions.
- **`files.version.cadence`** — Checkpoints at resting points, never per write; any version can be named later.

## Non-goals

- Server-side and end-to-end encryption
- External-storage backends (SMB/FTP/SharePoint/…) — storage locations cover this better
- WOPI / Collabora office editing — may return later
- Nextcloud's reshare rules and OCS API shape
- A workflow/automation engine, structured-data tables, dashboard widgets
- Automatic merging of application project files — keeping both is the answer
- Hard locks that block work or reject offline writes
- DRM, watermarking and secure-view — the deliverable concern, see [#14](14-deliverable-preview.md)
- Compliance certification as a feature (GDPR/HIPAA/…) and the audit surface — see [#2](02-single-trusted-system.md)

## Current state

Content-addressed chunk store (FastCDC + BLAKE3 over iroh-blobs) with jj for history; File Roots with pointer stubs; replica sync engine + daemon; WebDAV; share links with capabilities and a file-request inbox; video/audio renditions and a review player with timestamped comments.

The dominant gaps: **`files.write.*`** — there are no write RPCs at all, so nothing can be created, moved or uploaded over the network; **`files.index.*`** — files have no search of any kind and no content is extracted beyond video/audio proxies; and **`files.access.*`** — sharing is public-link-only, with no way to grant a colleague a folder. Per-capability detail is in the parity matrix.

_(Current state as of 2026-08-15, when this was an issue; the spec's coverage is the live answer.)_

## How we evaluate

Each requirement gets a yes/no against current behaviour, re-checked as the system evolves. Coverage against the code is tracey's `tracey_uncovered` / `tracey_untested`.

Related: [#13 The Ideal Project Organization](13-project-organization.md), [#14 The Ideal Client-Facing Deliverable Preview](14-deliverable-preview.md), [#17 Searchable - Anywhere - Anything](17-search.md), [#22 Self-Hostable - Federated Servers - Peer to Peer](22-self-host-federation-p2p.md).
