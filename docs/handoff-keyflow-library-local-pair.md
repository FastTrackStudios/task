# Handoff: the Keyflow library UI, and the local Keyflow ↔ Task pair

Written 2026-09-22. Two repos, two local commits, nothing pushed. Read
the **Before you push** section first — the Keyflow commit does not
build until the Task one lands.

## What this was

Keyflow's `/library` page could list songs and create a song list. It
could not find a song among two hundred, move one up, rename a list, or
delete one — and an account with no workspace hit a dead end. Fixing
that needed two RPCs Task did not have, so this spans both repos, and
the only honest way to work on it was to run the pair locally against a
real sign-in.

## State of the two repos

**Task** — `/run/media/Development/task`, branch
`feat/collection-rename-delete`, commit `5835172`, branched off
`origin/main` (`c0f5239`). Working tree clean apart from this document.

- `collection-proto`/`collection`: `rename` and `delete`. `rename` keeps
  the id, kind, org and every item — a set list renamed the morning of
  the show is the same set list, so links and subscriptions to it
  survive. `delete` removes the collection only; the nodes it gathered
  are untouched, because a collection names things it does not own.
- `apps/server/src/permits.rs`: `wr "rename"`, `wa "delete"` on the
  COLLECTION table.
- Root `Cargo.toml`: facet and vox pinned **exactly** (`=`). See
  *The version trap* below.
- `cargo nextest run -p collection` → 11/11. `ci-fmt`, `ci-manifests`
  and `cargo check -p xtask` pass. **`just ci` has not been run in
  full** — do that before pushing.

**Keyflow** — `/run/media/Development/fts/keyflow`, branch `main`,
commit `a99952b`. Working tree deliberately dirty; see below.

- `apps/web/src/routes/library.rs`: search box (title, writers, exact
  key), per-row ↑/↓, inline rename, two-step delete, row numbers.
- `apps/web/src/library/{vox,mod}.rs`: `rename_songlist`,
  `delete_songlist`, `move_in_songlist`, `ensure_personal_org`, plus a
  second dial lane for the server-lane call (`SERVER_LANE = "/server"`).
- `shelf_for`'s `OrgTarget::None` arm provisions the personal org
  instead of answering `NoOrg`.
- `Justfile`: `just web` now passes `--fullstack`.
- 112/112 `keyflow-web` tests pass.

## The dirty working tree in Keyflow is on purpose

`Cargo.toml` and `Cargo.lock` there carry a local block:

```toml
[patch."https://github.com/FastTrackStudios/task"]
resources-proto = { path = "/run/media/Development/task/features/resources/resources-proto" }
collection-proto = { path = "/run/media/Development/task/features/collection/collection-proto" }
links-proto      = { path = "/run/media/Development/task/features/links/links-proto" }
org-proto        = { path = "/run/media/Development/task/features/org/org-proto" }
```

That is what lets the site speak to a locally-served task-server whose
protos have moved past the pinned `main`. It is machine-specific and
must not be committed — **do not `git add -A` in that repo**. The
committed manifest points at `branch = "main"`, and a copy of both files
with the block is saved at `/tmp/claude-1000/keyflow-Cargo.{toml,lock}.withpatch`
if you clobber it.

## Running the pair

```bash
# Task, port 18080 — real sign-in against the deployed issuer
cd /run/media/Development/task
TASK_CENTRAL_AUTH_URL=https://auth.fasttrackstudio.app just demo serve

# Keyflow, port 8080
cd /run/media/Development/fts/keyflow/apps/web
KEYFLOW_TASK_URL=http://127.0.0.1:18080 \
  dx serve --platform web --ssg --fullstack --force-sequential
```

Port 8080 is not a preference: `http://localhost:8080/auth/callback` is
a registered redirect URI on fts-auth, and sign-in fails on any other
port.

Both processes belong to the agent — launch them with `run_in_background`,
tee to the scratchpad, read the logs. Kill them by recorded PID; never
`pkill -f "dx serve"`, which matches the user's own panes. Two `dx serve`
processes on one target directory clobber each other's fingerprints, so
confirm the old one is gone before starting a new one.

## Three traps, each of which cost an hour

**The version trap.** facet's reflection is what encodes the vox
handshake schema. A client on facet rc.7 dialling a server on rc.5 does
not negotiate — it fails with `schema decode consumed 252 of 782 bytes`,
which reads like a vox bug and is not one. A caret range over a
pre-release (`0.50.0-rc.5` alone means `>=0.50.0-rc.5, <0.51.0`) lets
either side drift on any unrelated `cargo update`. Both repos now pin
with `=`, and pinning caught a live instance: Task had already drifted
`vox-codegen` to rc.6. Bump the fleet together or not at all, and
resolve with one full `cargo update` — piecemeal `cargo update -p` fights
itself and broke `phon` when it was tried.

**The hydration trap.** Keyflow's `web` feature turns on
`dioxus-web/hydrate`, so the client always expects a hydration payload.
Without a server half rendering one, the site is a blank white page with
a single `atob` `InvalidCharacterError` in the console and no other
clue. `--ssg --fullstack` is mandatory, which is why `just web` carries
`--fullstack` now.

**`after: None` means the tail.** Task's `reorder` documents
`after: None` as *append to the end*, not *put it first*. The first
draft of the move buttons read it the other way and sent row 2 of a
204-song list to position 204. `move_plan` in `routes/library.rs` now
names the song a move lands after — for `at == 1` it demotes the first
song rather than naming no predecessor — and
`every_move_names_the_song_it_lands_after` pins it. The demo data that
incident disturbed was restored (`INSIDE OUT` back to rank `'mmmNm'`,
`.bak` kept beside `collections.jsonl`).

## What was verified in the browser

Against local Task, through a real OIDC sign-in, at
`http://localhost:8080/library`:

- Rename — "Adult Jam" → "Adult Jam 2026", 204 songs intact, survived a
  reload, renamed back.
- Delete — a throwaway list created and deleted through the two-step
  confirm; the library still holds 204 songs, so the collection went and
  the songs stayed.
- Search — `maroon` narrows 204 → 2, matching a title and a writer.
- Reorder — ↑ on row 2 swaps rows 1 and 2, leaves row 3 alone, and
  survives a reload.

## Before you push

1. Land Task first. `just ci` in full, then
   `gh pr create --fill && gh pr merge --auto --squash`.
2. Then Keyflow: `cargo update -p collection-proto -p org-proto` to take
   the merged Task, with the local `[patch]` block **removed** for that
   resolution, and confirm `cargo check -p keyflow-web --target
   wasm32-unknown-unknown` before pushing. The committed manifest tracks
   `branch = "main"`, so until Task merges, Keyflow's commit references
   RPCs that do not exist on the branch it pins.
3. Restore the `[patch]` block afterwards if you are still iterating on
   the pair.

## What is not built

Discussed with the user, not started:

- Song detail — arrangements and charts per song, reached from a row.
- Bulk add to a list, rather than one `Add to…` select at a time.
- Sort controls on the library (by title, writer, key, date added).
- A workspace switcher that is always visible, so a person in several
  orgs can see which library they are looking at.

Task's side of these is mostly there already; the work is Keyflow UI
against existing RPCs. Check `docs/app-integration.md` before adding a
new RPC — the lane and permit conventions are set out there, and every
new method needs a permit row or it is refused once
`TASK_ENFORCE_PERMISSIONS=1`.
