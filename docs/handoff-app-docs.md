# Handoff: app documents (`crdt::docs`) — Task side and what follows

Written 2026-10-24 on airlock, handed to THEBATTLESHIP: long cargo builds on
airlock kept losing their session. Branch `feat/no-issue-app-docs`, head
`ff528d81` (from `origin/main` `a322e20a`). Read `AGENTS.md` and
`skills/ship-stack.md` first.

## Goal

Any app (Studio first) keeps its data as Loro docs and syncs them through
Task across a person's devices, offline-first, and Task never reads a doc.
The machinery is architect's `crdt::docs`, released in **architect
v0.9.28** (PR FastTrackStudios/architect#79, tag `v0.9.28` → `f5edc67`):
- `Docs` service, `DocsStore` (`InMemoryDocsStore`,
  `crdt_seaorm::SeaOrmDocsStore`);
- `DocsHost`;
- `DocsDriver` with `DocsIndex` / `TursoDocsIndex` and `DocStorage`;
- `SyncedDoc::durable()`, `CrdtDoc::open_shared`.

Recipe D in architect `skills/architect/references/crdt-peer-sync.md`.

## What this branch does (committed, pushed, no PR yet)

- Every architect pin is bumped to `v0.9.28`, including
  `features/live/live-proto`, and `crdt-seaorm` is a new workspace dep.
- `OrgRoot::app_docs_db()` returns `<org>/app_docs.sqlite`.
- `apps/server/src/app_docs.rs`:
  - `open(db)` builds a `DocsHost` over `SeaOrmDocsStore` and
    `SeaOrmPersistence`;
  - `OrgDocSync` sends app docs to the host and everything else to the
    vault.
- `OrgAppState.app_docs` is opened beside prefs in `build_org_state`.
  - DocSync order: live (`live::DocSyncRouter { live, rest, access }`),
    then app docs, then vault.
  - `Docs` and its stream are mounted in `org_layer_router`.
- Permits `APP_DOCS` (rd get/list, wr create/rename, wa delete) and
  `APP_DOCS_STREAM` (rd changes), mounted as `core`.
- Seed: three docs in `example_org::DECLARED_APP_DOCS`, planted by
  `plant_app_docs` from `demo_cli`:
  - a setlist;
  - a session;
  - a branch of that session, with `parent`.

  Covered by `declared_tests::declared_app_docs_plant_once_with_their_lineage`.
- Tests in `apps/server/tests/it/app_docs_e2e.rs`:
  - two devices converge, a delete reaches both, and the docs survive a
    server restart;
  - an enforcing server refuses anonymous callers on list, create,
    changes and sync.
- Spec `offline.app-docs.*` in `docs/spec/offline.md`, gaps in
  `docs/spec/unmet.md`, step 11 in `docs/app-integration.md`, change file
  `.changes/added-app-documents-catalog.md`.

Verified on airlock:
- `cargo build -p task-server --tests`;
- `cargo test -p task-server --test it app_docs` (2/2);
- `--lib declared_tests` (40/40);
- `--lib permits` (7/7).

**Not green yet:** `just ci-local` failed at `ci-tests`. The step that
broke is `cargo test --no-run --workspace`, which compiles every crate's
tests (code 101). The error was not read. The likely cause is another
workspace crate that the v0.9.27 → v0.9.28 bump or the `DocSyncRouter`
field rename (`vault` → `rest`) breaks; `task-server` itself builds. Run
`just ci-local` and fix what it reports.

## Next, in order

1. **Ship the Task PR.**
   - Get `just ci-local` green, push, and open a draft PR into `main`
     titled `feat(server): app documents per org (crdt::docs)` (template
     filled), then link it.
   - Merge when `PR checks` passes; Cody approved releasing this work for
     use.
   - Delete this handoff file in the PR, or keep it as the record.
2. **Architect v0.9.29: device as host.** The design is agreed with the
   Studio agent.
   - **`driver.host(admit)`** returns a `DriverHost` that implements
     `Docs`, `DocsStreamSource`, `DocSync` and `DocPresence`.
     - It is backed by the driver's own index and open docs, so there is
       no second writer.
     - `admit: Fn(&Principal) -> bool` is checked on every session,
       never cached.
   - **The index.**
     - `DocsIndex` gets a device-local contiguous `seq` per row, which is
       the peer lane's `rev`. Task's canonical `rev` is never overwritten
       from a peer.
     - Merging a peer row: adopt it if unknown, last writer wins on
       rename (by `updated_us`), and a tombstone wins.
     - A doc a peer registers stays pending until registered with Task.
   - **Several lanes.**
     - `run` runs one session per lane key (Task and each peer),
       concurrently.
     - Cursors are keyed by lane. `TursoDocsIndex` migrates its existing
       cursor to the key `task`.
     - Each doc gets one `SyncedDoc` per lane.
   - **The relay.**
     - Bytes imported from lane X are forwarded to every other lane, not
       back to X, through an injectable outbox sender on `SyncedDoc`.
       Imports don't fire local-update subscriptions, which is why this
       is needed.
   - **`relay_url: Option<String>`** on `DeviceRegistration` and
     `PeerDevice` (`features/auth/auth-proto/src/device.rs`). The device
     registry itself already exists: `DeviceService` at fts-auth.
   - **Test:** devices A and B are each connected to Task and to each
     other. A edits offline, then reconnects to B only, with Task down. B
     gets the edit. When Task returns it gets the edit from either side,
     and the histories converge.
3. **The bump pair.** Studio needs ONE architect source in its lockfile.
   - Bump task@main to v0.9.29.
   - Open a keyflow PR, `build(deps): architect v0.9.29, task@main` (the
     same pattern as keyflow#33). Do it in a keyflow worktree: airlock's
     checkout is on `chore/editor-v0.1.24`, someone else's work.
   - Merge it and tag keyflow `v0.2.10`. Skip a keyflow tag on v0.9.28;
     the Studio agent asked for that.
   - Then tell the Studio agent (thread `studio-fa` on airlock, via Cody)
     the task@main rev and the keyflow tag. Studio bumps once.

## Agreed plan after that (send to Cody before starting)

- Blobs layer in architect.
  - BLAKE3 everywhere; SHA-256 only through an alias table.
  - Transfer by iroh-blobs between nodes, plus a vox `Blobs` lane.
  - Refs reported per device. A ref grants read only for a blob already
    in the SAME owner space, and `report_refs` needs Write. Test the
    cross-org refusal.
  - Pins and GC with a grace period.
- Then the Files port into architect, roughly 61k lines in reviewable
  PRs, with Task working at each step.
- Then the File Provider and mount code for macOS, iOS (first, for the
  iPad), Linux FUSE, then Windows and Android.
- Task is the host that registers the mount and runs the engine for every
  app. App Group: `group.app.fasttrackstudio.task`.
- Constraints from the Studio side:
  - on iOS the foreground app wins the engine lock and never blocks;
  - nothing in the engine touches the audio thread.
