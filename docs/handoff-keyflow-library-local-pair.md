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

## Second session (2026-09-22, later) — the four open items, built

All in Keyflow, **uncommitted** in its working tree (plus the untracked
`apps/web/src/routes/song_page.rs`). No Task change was needed: every
call already existed (`song`, `upsert_song`, `delete_song`,
`list_charts(song)`, `add_item`).

- **Song page** `/library/:org/song/:slug` — reached from a song's title
  on the shelf, and from a new "Song" link in the editor when a bound
  chart belongs to a song. Lists the song's charts (Open, Make main,
  two-step Delete), adds an arrangement (copies the main chart under an
  explicit slug `<song>-<label>`, because a derived slug would overwrite
  the chart being copied), shows and adds to song lists, edits
  title/writers/key, removes the song.
- **Bulk add** — tick songs (or "Select all shown"), pick a list; sends
  only what the list lacks and says how many it already had.
- **Sort** — Title / Writer / Key / Recently changed, remembered in
  `keyflow.library.sort`. `SongEntry` now carries `updated_at`.
- **Workspace always named**, even with one org.
- Save from a plain editor with no org now provisions the personal org
  (it still answered `NoOrg` there; the shelf already didn't).
- A chart whose song was deleted is listed with the unattached charts
  (it was on the server and nowhere on screen).

**A real bug fixed on the way:** `chart_doc_from` sent `is_default: true`
on every save of a chart attached to a song, on the belief (in its own
comment) that the server keeps the existing default. It does not —
`upsert_chart` treats the flag as a request that *wins*. So editing any
secondary arrangement, or adding one, stole the song's main chart. Now a
save sends no opinion (`Draft::make_default`, false by default) and only
"Make main" asks; the server already makes a song's first chart main.

Verified in the browser against local Task: edit details (written to
`assets/songs/africa.md`), add arrangement → bound editor, Make main,
delete arrangement, bulk add (3 added; then 1 added / 2 already there),
sort persisted, save from `/editor` → new song → remove song → its chart
under Charts → removed. Demo data restored afterwards (AFRICA key blank
again; its main chart now carries `key`/`sections` read from its own
source, as any save would write). 119/119 `keyflow-web` tests.

### Two more dev-loop traps

**Run Keyflow in Keyflow's shell.** From Task's dev shell, dx finds
wasm-bindgen 0.2.127 and refuses the build (Keyflow pins 0.2.126). Use
`direnv exec .. dx serve …` from `apps/web`. Also pass
`--hot-patch false`; hot-patching failed its rebuilds here.

**Stale pre-rendered HTML kills hydration silently.** dx serve re-runs
SSG on each rebuild but did not overwrite the existing files under
`target/dx/keyflow-web/debug/web/public/**/index.html`, so fresh wasm
hydrated against HTML from a morning build. Symptom: on a *direct load*
of `/editor` the whole Source pane (Save, Vim) ignores clicks, with no
console error, while the same page reached by in-app navigation works.
Fix: stop dx, `find …/public -name '*.html' -delete`, start dx again.
And never let two `dx serve` processes share the target dir — a leftover
one caused the "linking with cc failed" on `iroh-relay` seen first.

## Third pass — the library redesigned (same day, uncommitted)

The user's verdict on the working library was "the UI is really bad";
the editor, guide and landing page are "perfect" and were not touched.
The library now matches them:

- `routes/shelf.rs` (new) is `/library`: a full-width working surface
  like the editor — a sticky rail (workspace, All songs, Loose charts,
  each song list with its count, new list) and one pane at a time. All
  songs is a real `<table>` (Song, Artist, Key, Charts, Open chart);
  bulk add is a bar pinned to the bottom that appears only while songs
  are ticked, replacing 204 per-row selects. A list is a numbered running
  order at a reading width, with its controls quiet until the row is
  hovered. `routes/library.rs` keeps Save, the bound editor, and the
  tested helpers the shelf calls.
- `routes/song_page.rs` puts the selected arrangement on engraved paper
  (`chart::Chart`, page shape) beside a sticky column of arrangements,
  lists, details and remove.
- Keys and sections use the editor's syntax colours (`--syn-meta`,
  `--syn-section`) in the mono face. Dates read as `22 Sep 2026`
  (`short_date`).
- **Preview column** (asked for next): at ≥75rem the shelf is three
  columns: rail, list, and the picked row's chart with every page
  engraved as separate sheets in its own scrolling column. Clicking a
  name previews it, ↑/↓ steps through the list with the preview
  following, and arrangements show as pills when a song has several.
  Below 75rem there's no preview, and a click opens the song page
  (`PREVIEW_QUERY` in `shelf.rs` has to match the CSS). The per-row
  "Open chart" links are gone; the preview carries Open in editor.
- Verified at a 2552px viewport and in `/devices` at phone widths.
  Found in passing, not fixed: signed in, the shared header's nav
  overflows a phone. That's the header, outside the library.

## Fourth pass: live multiplayer charts, and the editor (uncommitted)

**Keyflow** (`apps/web/src/collab.rs`, `routes/editor.rs`,
`keyflow_editor.rs`): a bound library chart joins Task's per-file Loro
document (`open_collab("assets:charts", "<slug>.md")`). The editor edits
only the ```` ```keyflow ```` fence; edits cross via Task's own
`extract_fenced` / `replace_fenced`. Local edits go in synchronously
through the editor's `on_transaction`; remote edits come back as
`"remote"` transactions; carets and names go over the presence channel.
Keyflow now patches phon / phon-jit / facet-core to the vendor tag as
Task does, because `open_collab`'s reply carries a `Uuid`. Verified with
two tabs, and with an outside write to the file merging into open tabs.

**Editor** (`/run/media/Development/fts/editor`, branch
`fix/caret-stays-on-its-line`, to be renamed): the "letters land on the
line below" bug was the editor's own. It had two causes, both in the web
input path:
- `bridge::handle_input` inserted a single typed run at the *stored*
  caret. After a click during a muted frame, that caret is stale, often
  the start of the next line. It now uses the browser's caret after the
  insert, and trusts the stored caret only if it is a valid reading of
  the diff (`insertion_point`, with unit tests).
- `applyPatch` called `disconnect` / `takeRecords`, which throws away
  typing the observer hasn't delivered yet. It now delivers that typing
  first when the patch doesn't change the text.

The same editor tree also fixes the editor's own test gate:
- `editor-mermaid` linked the crates.io renderer, which panics on wasm
  (`Instant::now`), instead of the vendored copy with the fix.
- A `cfg(feature = "cli")` in the vendored copy was on the wrong line.
- The 8 command-menu tests were stale after the `\` trigger change.
- The Justfile defined `check` twice, and its wasm check needed
  `--features web`.
- Playwright: 72/72. `cargo nextest`: 750/751. The one failure,
  `dense_flowchart_keeps_mid_span_edge_reasonably_direct`, fails at
  upstream v0.2.2 as vendored (it isn't fonts) and is left for a
  decision.

Keyflow's `Cargo.toml` also carries a local `[patch]` block for the editor
checkout, beside the Task one. It is machine-specific and must not be
committed.

## What is not built

- Reordering a song list by drag (↑/↓ only).
- Editing a song's tags (kept untouched on save, never shown).

Check `docs/app-integration.md` before adding a new RPC — the lane and permit conventions are set out there, and every
new method needs a permit row or it is refused once
`TASK_ENFORCE_PERMISSIONS=1`.
