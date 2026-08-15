# Media roots at scale: versioning without duplication, binding without names

**Status:** designed 2026-08-14; both fixes BUILT the same day. What is
left is operational, not code — see "Sequencing" at the bottom.

Two blockers found while putting ~5 TB of real production media under
Task on a NAS with 1.1 TB free.

## 1. The first checkpoint copies everything

A media root's checkpoint streams file content into the CAS chunk store
(`content.rs`: media roots "write-then-compare, where the chunk store
dedups"). The first capture of a root therefore writes a **second copy**
of every byte. For one 268 GB `.braw` that is 268 GB; for the tree here
it is several terabytes that do not exist.

That is why registration currently has to be paired with either "never
checkpoint" or "ignore all media" — both of which give up version
control on exactly the files people most want to recover.

### The fix: reflink the content in

`/mnt/storage` is XFS with `reflink=1`, **verified** (`cp --reflink=always`
of a 200 MB file consumed no space). A reflink clone shares extents and
only duplicates blocks when one side diverges.

So the first checkpoint can cost metadata only, while keeping history
honest:

- live file unchanged → one set of blocks, two references
- live file edited → XFS copies only the changed extents; the
  checkpoint keeps the originals, so old versions remain real
- live file deleted → the store's reference keeps the blocks alive

Shape: in the CAS write path, a file above a threshold (~64–256 MB) is
stored **whole, via `FICLONE`**, rather than FastCDC-chunked. Small files
keep chunking.

**Built** as `ChunkStore::write_path` / `probe_path`, threshold 64 MiB.
Two things made it smaller than designed:

- iroh-blobs' own `ImportMode::Copy` *already* reflinks (its import path
  tries `FICLONE` and falls back to a copy), so no new storage tier was
  needed — just importing by path instead of streaming bytes in.
  `TryReference` was rejected: it points the store at the live file, so
  a later edit would destroy the version meant to survive it. `Copy` is
  clone-then-own, and CoW handles divergence correctly.
- A whole-stored file is a one-entry manifest whose hash is the file's
  blake3, so the manifest format, GC liveness and read paths are all
  unchanged.

Measured on btrfs, 256 MiB of incompressible data: **258 MiB consumed
chunked, 1 MiB stored whole** (the residue is the bao outboard, ~0.4%).
The measurement needed a `sync` and incompressible content to mean
anything — with delayed allocation and `compress=zstd` a full copy reads
back as free.

Two consequences worth knowing:

- `read_range` no longer buffers a whole chunk — with one blob per file
  a `<video>` seek would have pulled the entire file into memory. It
  seeks the blob and copies only the window.
- `probe_path` must make the *same* size decision as `write_path`, since
  a whole-stored file and a chunked one have different ids for identical
  bytes. Getting that wrong would make every capture re-import every
  large file forever; a test pins it on both sides of the threshold.

The trade is losing chunk-level dedup between versions of a large file.
For media that is nearly worthless — a re-encode changes every byte —
while cheap snapshots of *unchanged* files are the dominant case by
orders of magnitude.

Requirement: source and store on one filesystem. Satisfied — a root's
version store lives in `.fts-files/` inside the root itself.

Fallback when `FICLONE` fails (different filesystem, non-reflink FS, an
older kernel): fall back to the existing streaming write, and say so in
the span. Silent full copies are how a disk fills at 3am.

## 2. Roots bind to projects by NAME

`org_tree.rs:130` matches `root.name == project_folder_name`, plus an
`Album — <name>` special case. Rename either side and the media silently
disappears from the project — no error, just an empty `Media/`.

Tonight produced three separate demonstrations:

- a folder whose name carries an invisible **U+F022** (private-use
  character from a Mac font) — an exact-name copy failed on it
- **three spellings** of one project ("El Artista Eres Tu", "El Artisa",
  "El Ariste De Tu")
- "Jaramillo" vs "Jaramillio", inconsistent within one project's own files

### The fix: bind by id

The project note carries the link, because the vault is the
human-editable source of truth and survives outside the app:

```yaml
media_roots:
  - 3f9c1e88-...
```

`projects_area` resolves by id, falling back to name-matching for
anything not yet linked — so nothing breaks mid-migration.

Two capabilities fall out that name-matching cannot express, and both
are already needed:

- **several roots on one project** — the Yokasta job is four separate
  piles (sessions, a Blackmagic reel, Canon A/B roll, a choir shoot)
- **one root on several projects** — the Village choir footage belongs
  to Goodness of God *and* Yokasta, which is why it is currently parked
  in an inbox rather than filed

## Registration order, once deployed

The tree is now `Projects/<org>/<project>`, six orgs, 37 projects. Four
of them are genuine containers whose children are songs — Crescendum,
Montreal Benefit, PNG Worship Collective (13 songs), Roger Garcia — and
several more (Campus Jax Shows, The Bible Game Gospel, ONE805) hold
sub-somethings whose status is a judgement call, so they start as one
root each.

**Decide nesting before a container's first checkpoint.** Carving a
child root out later is *safe* — the album keeps its own files, the
song root takes over, and everything already captured stays recoverable
via `browse_at` on the pre-split commit (pinned by
`carving_a_child_root_out_of_tracked_files_keeps_the_history`) — but the
album records a deletion of those paths at the split, and `chain()`
stops answering for them: it walks back from HEAD and halts where the
path is absent (`chain.rs:112`), which is how *every* deleted file
behaves, not a carving quirk. Registering the songs first avoids the
whole question.

Roots have a driver, unlike grants: `task files root create` with
`--server` / `--org`, so this is a scripted pass, not new code.

## Sequencing

1. ~~**Bind by id**~~ — done.
2. ~~**Reflink storage**~~ — done.
3. **Deploy, then register.** What remains is operational: the image
   carrying both fixes plus `TASK_STORAGE_VOLUMES`, then the Storage
   Location, the per-org grants (CBU vs TomBrooksMusic path prefixes),
   and the roots themselves.

   There is more room here than it first looked: the cadence engine is
   **activity-driven**, not periodic (`cadence/engine.rs` — a session
   opens on a write hint, snapshots after 10 min of uncaptured activity,
   checkpoints 30 min after activity stops). A freshly registered root
   nobody is writing to captures nothing at all, so registration is not
   itself a trigger.

   Still, register the first root and then **watch free space on
   `/mnt/storage` before adding the rest**. The reflink verification so
   far is btrfs here and a `cp --reflink=always` probe there; what has
   not been observed is task-server's own writes landing as clones on
   that mount. The store lives in `.fts-files/` inside the root, so it
   is on the same filesystem as its source — the requirement — but the
   array has 1.1 TB free against several TB of media, and a wrong
   assumption fills it before anyone notices.

## What is registrable today

Nothing yet, for a third reason: Storage Locations are admitted from
volumes an agent **announces**, and the in-server agent announces only
`primary` under the data root (the PVC). A NAS mount is invisible to the
registry no matter how it is granted. `TASK_STORAGE_VOLUMES`
(`key=/abs/path`, comma-separated) fixes that by announcing extra
volumes at boot; missing paths are skipped with a warning rather than
failing the boot.
