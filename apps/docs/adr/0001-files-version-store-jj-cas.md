# Files version store: jj-lib on a custom CAS backend

Files needs automatic per-file version chains, no-locking Divergent
versions, session-checkpoint cadence, and multi-GB media dedup — plus
real git for software projects. We chose **jj-lib as the single version
engine for every File Root**, with a Files-owned content-addressed
backend for media roots and stock colocated-git for software roots,
rather than a split media/software hybrid or a CAS-native engine built
from scratch. The decider: jj's op-log concurrency, divergent changes,
and first-class `Merge<TreeId>` conflicted trees are a shipped, tested
implementation of exactly our Divergent-versions semantics, and jj never
inspects file bytes, so one 15-method backend trait serves 4 KB text and
40 GB video. Research: `docs/research/jj-as-files-version-engine.md`,
`docs/research/files-rust-crate-landscape.md`; decision record:
FastTrackStudio issue #228.

## The decisions

- **Engine**: jj-lib (pinned, vendor-patched per monorepo policy; it is
  pre-1.0) with a custom `Backend`. Rejected: hybrid (two version
  graphs, two divergence models, a "which files are media" router buys
  nothing) and CAS-native (re-implements jj's hardest 20%).
- **CAS substrate**: fastcdc (v2020) chunking + blake3 + iroh-blobs
  `FsStore`. `FileId` = hash of a chunk manifest; chunks are blobs.
  iroh-blobs is treated as beta: our own chunk manifests live outside
  it so the store is rebuildable. Shared blake3 keeps us wire-compatible
  with the iroh transfer stack for sync.
- **Root flavor at creation**: `media` (custom CAS backend, the default)
  or `software` (colocated git — a perfectly normal `.git` for GitHub,
  CI, IDEs). Doctrine: big media belongs in media roots; software roots
  keep real git and ignore-pattern stray heavy files. Flavor conversion,
  if ever wanted, is a deliberate relocation-style operation.
- **Named Versions and Project Versions are Vault entities**, not engine
  constructs: a Named Version references `(root id, change id)`; a
  Project Version references the root plus the commit starting the
  iteration (same root, new lineage — no new root, no "Project NEW"
  folders). The version store knows nothing about names.
- **Authoritative repo placement**: the Storage agent hosting a root's
  live tree owns the authoritative repo (it also runs the watcher —
  inotify is blind on NFS clients, so change detection is server-side).
  Blob placement across Storage Locations is a separate axis. Client
  repo/workspace mechanics belong to the sync daemon design.
- **Divergence is resolved in the API/UI, never as on-disk conflict
  markers**: the live tree shows the newest save; both sides survive in
  the store under one change id; a divergence badge rides the root and
  the file's chain until resolution ("pick A / pick B / keep both").
  This also dodges jj's whole-file buffering on marker parsing.
- **GC**: the backend implements `gc(index, keep_newer)` (mandatory).
  Protect set = index-reachable ∪ Vault-referenced (Named Versions,
  Project Version starts, share-link targets, review pins) — the Vault
  is the authority on immortality. Unnamed checkpoints expire per-root
  retention policy; the retention numbers are set with the versioning
  cadence, not here.
- **Recorded renames in backend v1**: our backend implements jj's
  `CopyHistory` so per-file chains are recorded fact, not heuristic.
  Detection may start simple; storage of copy records ships early
  because retrofitting it after history exists is a bad migration.
- **Per-file chains are derived** by walking the DAG with path filters
  (jj's supported pattern). A path→commits cache is added only if a
  root's snapshot count makes the walk measurably slow — never a second
  source of truth.

## Consequences

- We own: a streaming chunked blob store behind the `Backend` trait,
  its GC, copy-record storage, and the sync transport (jj supplies
  merge semantics only).
- Backend policy: `snapshot.max-new-file-size = 0` (or per-root),
  checkpoint-cadence snapshots rather than per-command, fsmonitor-style
  incrementality for big roots.
- Checkout is full-rewrite (no reflink) in jj today — accepted for v1.
