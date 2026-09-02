# Task — domain glossary

Vocabulary only. No implementation detail; decisions live in ADRs and
on their tickets.

- **Vault** — the small, fully-replicated layer: markdown notes and
  structured overlays (tasks, projects, events). Lives in full on every
  device a user is logged into. Offline-first, multiplayer. Countable:
  an owner may hold several, though one is the norm.
- **Wiki** — a vault built on the same primitives, differing in reach:
  it is publishable, so others may subscribe to it, link into it, and
  ask to change it. Owned by an org — a person's own wiki is owned by
  their own org. A vault is not subscribable; a wiki is, and that is
  the whole of the difference — same primitives, same tree, different
  name. A vault may be promoted whole into a wiki, or a single note
  contributed to one, as suits.
- **Subscription** — holding another org's Wiki or a Resource such that
  its content resolves inside your own writing: links, search, graph,
  embeds. Held by a vault or a wiki, never granted by a share link.
  Not transitive — what a source you subscribe to subscribes to does
  not become yours — though what you already hold resolves regardless
  of who wrote the link.
- **Resource** — an external work pulled in and archived to be
  referenced later: a book, a video, a paper, a scripture text. Read,
  annotated and cited; never authored here, and never edit-requestable.
  Most are *personal* — archived under someone's own rights, and
  therefore not shareable outward. A *published* Resource is one whose
  rights permit redistribution, and only those can be subscribed to.
  The Bible Resource is the text itself; the Bible Wiki is the curated
  collection *about* it. Both are subscribable, and they are different
  things. A Resource carries no annotations of its own: everything
  anyone says about it lives in a wiki or a vault, anchored to the
  Resource's canonical address (verse, timecode, page) rather than to
  its text.
- **Editor** — a role on one wiki: may accept changes into it,
  including their own. Granted by that wiki's owner or an org admin.
  Distinct from org `admin` / `member`, which say nothing about any
  particular wiki.
- **Adoption** — claiming a wiki whose publishing org is gone, so it
  keeps a home and its references keep resolving. Offered by a
  departing org, or claimed from an orphan. The wiki keeps its
  original qualified name; the org registry holds the redirect. If
  nobody adopts, subscribers simply keep their local copies.
- **Edit Request** — a proposed change to a wiki from someone without
  the Editor role on it: the changed pages themselves, against a named
  version, not a description of them. It *is* an issue on the owning
  org's tracker, so closing the issue and resolving the request are one
  event. An Editor's own change is not exempt from the lane — it is
  auto-approved within it, so every change to a wiki has one trail.
- **Files** *(working name — final term unsettled)* — the large binary
  layer: audio/video/project media. Lives primarily on servers;
  reaches devices only by selective sync or NAS-transparent access.
  Distinct from the Vault: the Vault indexes it, never contains it.
- **Selective sync** — placing a replica of a chosen root or root
  slice onto a device for local work, with the root's ignore set
  applied. The opposite of the Vault's everything-everywhere
  replication.
- **Replica** — a device-local copy of a root or slice, hosted by that
  machine's storage agent as a real live tree with its own version
  store: offline edits checkpoint locally and reconcile later, possibly
  as divergent versions. May be partial (pointer stubs, hydrate on
  demand).
- **NAS-transparent access** — using server-resident Files in place
  over the network (NFS today) as if local, without syncing them.
- **File version chain** — the automatic, per-file history of every
  saved state of a single file. Safety net, not user ceremony: nobody
  stages or commits.
- **Named Version** — a user-facing, deliberately labeled version of a
  deliverable ("v3 for client"). Curated on top of automatic chains.
- **Project Version** — a whole-project iteration. Restarting a project
  creates a new Project Version of the same project, replacing the
  "Project Title old" / "Project Title NEW" folder idiom: the folder
  name never changes. Auto-numbered (v1, v2, …) with an optional label;
  old iterations stay browsable read-only and files can be copied
  forward into the current one.
- **Divergent versions** — when two machines save the same file
  concurrently, both saves survive as sibling versions to be merged or
  chosen later. There is no locking and no lost data.
- **File Root** — a folder tree with its own identity: a first-class
  vault entity (own note) that projects *reference*, never own. Roots
  never overlap on disk — one tree, one root, versioned once. A root's
  live tree sits wholly on one Storage Location; its version-store
  blobs may be placed across locations. Identified by a stable id in
  its entity plus a marker file in the tree; the (location, path)
  binding is mutable. Roots may live anywhere, including inside a
  vault folder — vault replication excludes root subtrees. Policies
  (versioning, retention, placement) live on the root itself.
- **Root flavor** — a File Root's versioning mode, chosen at creation:
  *media* (the default) or *software* (a real git repo, fully usable by
  git tooling). Doctrine: big media lives in media roots; a software
  root ignores stray heavy files rather than versioning them.
- **Version store** — the engine and storage behind version chains,
  checkpoints, and divergent versions for a root. Authoritative beside
  the root's live tree; names and curation (Named Versions, Project
  Versions) live in the Vault, never in the store.
- **Root slice** — a reference to (root, subpath): how subprojects,
  share links, and note-embedded widgets point at part of a root
  without creating a nested root.
- **Drive** — the raw, NextCloud-style browsing surface over Storage
  Locations: loose files outside any root. Projects are a convenient
  view over Files, not a cage; a per-user Home root covers personal
  files that still deserve versioning.
- **Storage Location** — a named place Files can live: a server volume,
  an S3 bucket, an external drive. Deployment-scoped: the operator
  registers locations; orgs reach them only through Storage grants.
  Each location declares its capability classes — hosting *live trees*
  (POSIX/NFS) and/or holding *blobs* (get/put) — and is spoken for by
  exactly one Storage agent. Task decides *placement* (which location
  holds what); physical tiering/redundancy below a location (SSD cache,
  RAID, ZFS) belongs to the substrate.
- **Storage grant** — an org's admission onto a Storage Location: a
  capability subset, a byte quota (counted logically — the bytes the
  org's roots reference, dedup savings belong to the operator), and a
  path prefix that is the org's own subtree on a shared volume.
- **Storage agent** — the process that speaks for a location: in-process
  in task-server for its own volumes, the desktop app's headless agent
  for a plugged-in drive, or a standalone agent on a NAS/storage box.
  One protocol, three hostings; agents announce their volumes, the
  operator approves. Agents, not the server, carry blob transfers
  between locations — the coordinator is never the data path.
- **Pointer stub** — a small placeholder file standing in for
  non-resident content inside a live tree (a dehydrated file). The
  agent hydrates on demand: explicitly, by root policy patterns, or on
  access through Task-mediated surfaces. Raw NFS reads a stub as a
  stub — no fault-in without FUSE.
- **Removable location** — a location on an external drive:
  replica-first (a tracked replica of server-primary roots for
  portable/offline work; offline edits reconcile as divergent
  versions), hosting a live tree only when specifically declared.
  Expected-offline is a health state, not an error.
- **Relocation** — the deliberate move of a root's live tree between
  locations: checkpoint, copy, verify, flip the (location, path)
  binding inside a declared unavailability window; the source is
  demoted to a read-only copy. Never automatic.
- **Session checkpoint** — the guarantee that matters: everything is
  versioned by the end of a working session. The durable, chain-visible
  version, minted when a root's session ends (quiescence or an explicit
  "checkpoint now") and certified by a full scan. Sessions are per-root:
  concurrent writers to one root share one session.
- **Auto-snapshot** — an ephemeral safety capture taken automatically
  during activity. Expirable, invisible by default, never a chain
  entry; it exists so a mid-session mistake is recoverable. A
  project-file save marks the nearest auto-snapshot as a **save point**
  (display metadata, not a version).
- **Ignore set** — the per-root list of patterns that are neither
  versioned nor synced (backup files, peak caches). Seeded by root
  flavor, edited per root; versioning and selective sync share it.
- **Share link** — the one link entity for everything shared outward:
  tracked, retroactively editable, disable-not-delete. Targets a note,
  a root slice, a Named Version, or a Review page. Carries capability
  axes (view / comment / download / file request) plus optional
  password and expiry. Write access is never link-based.
- **File request** — a share link capability letting anonymous
  visitors *add* files into the target slice; never overwrite or
  delete. Uploads land in a per-link incoming area the owner promotes
  from, attributed to the link.
- **View-only** — a share link without the download capability:
  no download affordance, media reachable only as streamed proxy
  renditions (originals are never sent). Deterrence, not DRM.
- **Review page** — a thin first-class entity presenting a media file
  (via root slice) for review: player plus timecoded comments and
  frame annotations. A review survives new versions of its file;
  every comment records the file version it was made on.
- **Migration source** — a legacy store (e.g. `nextcloud-data`) that
  content is imported *from*; never written back to. NextCloud runs
  alongside indefinitely; it is not decommissioned by this effort.
