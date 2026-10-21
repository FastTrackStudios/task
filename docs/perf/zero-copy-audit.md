# Zero-copy audit

Where the server copies data it could borrow, share or stream, crate by
crate, ranked by **bytes × frequency**. This is the work list for the
zero-copy port (card t_2274242e / STA-84). It follows the memory work in
`docs/server-memory.md` (t_37712151), which measured the baseline and set
the budget this port tightens.

Each PR in the port takes one crate or area from this list. A PR updates
this file: it ticks off what it fixed, adds what it found, and records
the seeded scenario's numbers before and after.

## Ground rules

- **No on-disk or wire format changes** in a port PR. A change that needs
  one (say a `vox` message carrying `Bytes` with a different encoding, or
  a manifest stored differently) gets its own ADR-style note in
  `docs/adr/` first.
- The patterns, in order of preference:
  1. a **borrowed view** instead of an owned copy (`&str`, `&[u8]`,
     `&Path`, an iterator over borrowed items);
  2. **move** instead of clone where the owner is done with the value
     (take by value, `into_*`, `std::mem::take`);
  3. **`Cow`** and `#[serde(borrow)]` where a value is usually borrowed
     but sometimes has to be rewritten;
  4. **`Bytes` / `Arc<[u8]>` / `Arc<str>`** for data with several owners
     (fan-out to subscribers, caches, the same blob on several lanes);
  5. **streaming or mmap** for anything whose size is set by a user's
     file rather than by us;
  6. **buffer reuse** in loops (one `Vec`/`String` cleared per iteration).
- A port must not trade memory for a correctness change. When a borrowed
  reading can decide differently from the owned one it replaces, a test
  pins both readings against each other (see `vault_entity::frontmatter`).

## How the ranking was made

- **Measured:** heaptrack over the seeded scenario of t_37712151 (seven
  orgs, a File Root, a syncing laptop, MCP traffic, device sweeps,
  12 minutes). Allocation counts below are from that run's
  `stacks-base-allocs.txt`, attributed to the innermost workspace frame.
  That run was taken **before** t_37712151's fixes, so the sites that PR
  already fixed are listed as done.
- **Estimated:** for sites the scenario does not exercise, bytes × calls
  is estimated from the code: the size of what is copied (a page, a
  file, a request body, a struct) times how often the path runs (per
  request, per page per request, per event, per sweep).
- **Lints:** the four copy lints at warn (`just clippy-copies`), counted
  over the workspace. t_37712151 counted 146 over task-server and the
  crates it builds: `needless_pass_by_value` 84, `redundant_clone` 57,
  `implicit_clone` 5, `large_types_passed_by_value` 0; by crate files 45,
  server 36, vault-live 10, then a long tail.

Sizes: "page" is one markdown file's text, "vault" is every page of an
org, "file" is anything a user drops in a vault or a File Root
(unbounded).

## The list

Status: **done** (landed, with the PR), **next** (this port, scheduled),
**later** (worth doing, not yet scheduled), **leave** (looked at; not
worth it, with the reason).

### vault-live / vault-obsidian / vault-entity / entity scans

The vault is re-read on every list call (every task, goal, milestone and
workstream list, every search), so a copy per page here is a copy of the
whole vault per request.

| # | Site | Copy | Bytes × frequency | Status |
|---|------|------|-------------------|--------|
| V1 | `task::parse::looks_like_task` | whole frontmatter → `serde_yaml::Mapping` per page | 7.5 M allocs / 12 min (the #1 app site) | **done** (t_37712151: visitor) |
| V2 | entity scans (`task`, `goal`, `milestone`, `workstream`) | `VaultPage::to_proto` cloned each page's text | vault × list calls | **done** (t_37712151: `into_proto`) |
| V3 | vault manifest, watcher, `If-Match`, collab ack | `fs::read` whole file to hash it | file size × manifest calls (755 k allocs in `sha256_hex` callers) | **done** (t_37712151: 16 KiB streaming hash) |
| V4 | `search_vault` | built the full manifest to match paths | vault × searches | **done** (t_37712151: `Backend::paths`) |
| V5 | `vault_entity::frontmatter::has_type` (goal, milestone, workstream, every `VaultEntityStore`) | whole frontmatter → `Mapping` per page, per entity list | 398 k allocs; page × pages × lists | **done** (PR 1: `TypeProbe` visitor, parity test against the mapping reading) |
| V6 | `task::scan::scan_vault` | `to_proto` cloned each *matching* task page, then `parse_str` copied path, basename and text again into a shim | task pages × 2 × task lists | **done** (PR 1: parse the borrowed page) |
| V7 | `vault_live::walker::walk_vault` | `to_path_buf` + `clone` of every path, lowered extension copy per file, three `stat`s per entry | 881 k allocs; files × walks | **done** (PR 1: classify in place, `into_path`, walkdir's file type) |
| V8 | `vault_live::vault::load_entry` | `rel_path` cloned out of a list dropped right after; a `PathBuf` built to split the name; two path resolutions per file | 367 k allocs; pages × opens | **done** (PR 1: entries consumed, `name_parts` borrows, one handle) |
| V9 | `vault_live::watcher::is_markdown` | lowered extension copy per raw fs event | 724 k allocs; per fs event | **done** (PR 1) |
| V10 | `vault_obsidian::walker::{walk_vault, is_hard_skip, classify}` | `Vec` of every path's components per entry; lowered extension copy; path copied | 482 k + 84 k allocs | **done** (PR 1) |
| V11 | `files_proto::consts::is_private_dir`, `shelf_boundary::is_nested_shelf` | `dir.join(marker)` per directory per walk | 231 k + 157 k allocs; small | **leave** for now: a `PathBuf` per directory to `stat`; only worth a reused buffer if the walk is still hot after V12 |
| V12 | `Vault::open` per list call | re-reads every page of the vault on every list | **vault × list calls**: the largest remaining churn in this area | **done** (PR 4: `Vault::snapshot`, one cached snapshot per root, stat-validated, invalidated by the vault watcher) |
| V13 | `VaultPage` / `VaultBase` derive `Clone`, `Vault` derives `Clone` | a cloned `Vault` copies every page | unknown; callers not yet audited | **done** (PR 4: `raw: Arc<str>`; a cloned page or vault shares the text, `create_page` takes `Into<Arc<str>>` so a rename moves no text) |
| V14 | `vault_live::sync` `put_file` / `move` paths | `fs::read` of source and destination to compare and hash | file size × writes | **done** (PR 5a: `put_file` already streamed its `If-Match` hash; `move_file` now hashes the source by streaming, copies it with `fs::copy` instead of a buffer, and reads bytes only for a refusal) |
| V15 | `lexorank` | `String::from_utf8(bytes.to_vec())` | tiny | **leave** |

### task-server (MCP, sharing, request plumbing)

| # | Site | Copy | Bytes × frequency | Status |
|---|------|------|-------------------|--------|
| S1 | `AppState::org(slug)` | clones the whole `OrgAppState` (strings, roots, plugin set, auth, sharing handles) out of the map on every request | struct × every MCP / HTTP call (`authenticate_for` 682 k allocs) | **done** (PR 2: `orgs` holds `Arc<OrgAppState>`, `org()` hands out the `Arc`) |
| S2 | `mcp::mcp_dispatch` | body `String` parsed to a `serde_json::Value`, then `params` and `id` deep-cloned out of it | request size × 2 × calls (1.69 M allocs) | **done** (PR 2: `Bytes` body + `from_slice`; `parse_request` / `split_tool_call` move `id`, `params`, `name`, `arguments` out; parity test against the copying reading) |
| S3 | `mcp::authenticate_for` | `std::env::var("TASK_MCP_TOKEN")` (a `String`) per call; token cloned per session lookup | per call | **done** (PR 2: `AppState::static_token`, read once per boot). The bearer is still cloned once per session lookup: `architect_auth::CurrentSession` takes an owned `String` |
| S4 | `mcp::call_tool`, `tool_ok`, `rpc_result` | tool results built as `Value` then serialized; the catalogue rebuilt per `tools/list` | 388 k + 73 k + 105 k allocs | **done** (PR 2: `tool_catalog()` is a `OnceLock`'d `&'static [ToolDef]`). `tool_ok`/`rpc_result` already move their values; the response is still one `Value` serialized by `Json` |
| S5 | `iroh_host::bind_org` | 22.7 MiB of the 67 MiB peak | retained, not churn | **leave** here: iroh endpoint and blob store state, owned by t_4219725c / t_911268fa |
| S6 | `sharing::{sources, subtree}` | path strings rebuilt per entry while walking a shared subtree | entries × share listings | **later** |
| S7 | 36 copy-lint hits (server) | mostly `needless_pass_by_value` on handlers' `String`/`Vec` args | small each | **done** (PR 2: 37 → 0 over lib, unit tests and the `it` binary; `task-server` denies the four lints in `lib.rs`, `main.rs` and `tests/it/main.rs`) |

### files / files-store / files-sync

The Files feature moves user files, so its copies scale with file size
rather than with request count — fewer calls, far more bytes each.

| # | Site | Copy | Bytes × frequency | Status |
|---|------|------|-------------------|--------|
| F1 | `ChunkStore::read_to_vec`, `Backend::sync_object`, `sync_read_chunk` | whole file / object / chunk into a `Vec<u8>` | file size × reads | **done** (PR 3: `read_chunk`, `read_raw_object`, `sync_object`, `sync_read_chunk` return `Bytes`; `read_to_vec` → `read_bytes`, one allocation sized from the manifest; whole-tier chunk reads sized from the file; imports take `impl Into<Bytes>` / a slice. Anything larger than a chunk was already streamed — `read_to`, `read_range`, `read_chunk_range`, bao windows — and stays so) |
| F2 | `files_sync` object cache (`HashMap<String, Vec<u8>>`) | each object held once per lane that fetched it | object size × lanes | **done** (PR 3: the commit cache holds `Bytes`; decoding on one blocking thread and importing on another share it instead of cloning the `Vec`. The vox wire stays `Vec<u8>`; `Vec::from(Bytes)` hands over a uniquely owned buffer without a copy) |
| F3 | `files_sync` `chunks(root_id, batch.to_vec())` | batch of hashes copied per request | small × batches | **done** (PR 3: each batch is split off the owned list and moved into its request) |
| F4 | `files::lane::federation::trim_to_byte_range` | full object read, then trimmed | object size × ranged reads | **later**: read only the range (seek + `take`) |
| F5 | `files::{durable, hydration, ignore}` `fs::read` | whole file to inspect it | file size × calls | **later**: stream where only a hash/prefix is needed |
| F6 | `files::backend` merge (`side_a.to_vec(), side_b.to_vec()`) | both sides copied to merge | file size × merges | **later**: merge from slices |
| F7 | `FileId::new(id.as_bytes().to_vec())` (content.rs) | 32-byte ids | tiny | **leave** |
| F8 | 45 copy-lint hits (files) | the largest lint cluster | small each | **done** (PR 3: 45 → 0; `files`, `files-store` and `files-sync` deny the four lints) |
| F9 | `FilesBackend::sync_has_object` | read the whole object to answer "is it here?", once per object a pull walks | object size × objects × pulls | **done** (PR 3: `VersionStoreBackend::has_raw_object`, a `stat`) |

### vault-doc (CRDT peer)

| # | Site | Copy | Bytes × frequency | Status |
|---|------|------|-------------------|--------|
| D1 | `peer`/`replica` update channels (`vox::Rx<Vec<u8>>`, `UnboundedReceiver<Vec<u8>>`) | each update fanned out by cloning its `Vec` | update size × subscribers | **moved** (PR 5a): the fan-out is `crdt`'s document registry, in `FastTrackStudios/architect`, not in this repo. vault-doc holds one copy per local update (Loro's `subscribe_local_update` lends `&Vec<u8>`, so the outbox must own it) and moves it to `up` without another clone. `Bytes` in the registry is an architect change |
| D2 | `load_updates` → `Vec<Vec<u8>>` | every stored update for a doc loaded at once | doc history × opens | **moved** (PR 5a): `load_updates` is `crdt::Persistence`, defined in architect and called by `CrdtDoc::open` there; `SharedStore` here only forwards it. Streaming needs the trait changed in architect, then each store (architect's `FilePersistence` and in-memory store, `ui::doc_replica` here) |

### wiki-live

| # | Site | Copy | Bytes × frequency | Status |
|---|------|------|-------------------|--------|
| W1 | 66 `fs::read`/`read_to_string` sites | whole pages read per operation | page × calls | **done for `list_pages`** (PR 4: the one site that read every page per call now reads the wiki root's snapshot). The rest read one named file each (a page, `purpose.md`, a source) or run once per ingest: **leave** until measured; `index::rebuild` (every page, per ingest) is the next candidate |
| W2 | 105 `to_owned` / 143 `clone` | mostly path and title strings | small each | **later**, with the lint |

### Bases (`vault_live::bases`, `vault_obsidian::base_query`)

| # | Site | Copy | Bytes × frequency | Status |
|---|------|------|-------------------|--------|
| B1 | query execution | evaluates over owned `Value`s of every page's frontmatter | vault × base views | **part done** (PR 5a): rows are built from the frontmatter map (`BaseRow::from_frontmatter`) where both vault paths printed it to JSON and parsed it back, and a base's views share one row set (`execute_view_on`) where each view got a `rows.clone()`. Parity tests pin both against the JSON reading. **later**: evaluate over borrowed frontmatter (`Cow<str>`), a rewrite of `BaseEval` |

## Guardrails

- `just clippy-copies [cargo args]` runs clippy with `redundant_clone`,
  `implicit_clone`, `needless_pass_by_value` and
  `large_types_passed_by_value` at **warn** and prints how many of each
  remain. It is a report, not a gate.
- The lints are not in `[workspace.lints]`: `ci-clippy` runs with
  `-D warnings`, so a lint at warn there fails every branch. Instead a
  crate that has been cleaned **denies** them in its own root:

  ```rust
  #![deny(
      clippy::redundant_clone,
      clippy::implicit_clone,
      clippy::needless_pass_by_value,
      clippy::large_types_passed_by_value
  )]
  ```

  Once every crate carries the deny, the four move into
  `[workspace.lints.clippy]` at `deny` and the per-crate attributes go.
- The memory budget (`apps/server/tests/it/memory_budget.rs`) is the
  regression gate for the seeded scenario. Its limits are tightened as
  PRs land; each PR below records the numbers it measured.

## PR log

### PR 1: the vault walk and the entity discriminator

Branch `perf/zero-copy-vault-live`. Fixes V5–V10.

- `vault_live::walker` and `vault_obsidian::walker`: classify by
  extension in place, move walkdir's path out of the entry, ask walkdir
  (not `stat`) whether an entry is a directory, take only the first path
  component.
- `Vault::from_entries` consumes its entries; `load_entry` moves the
  relative path into the page, splits name and folder as borrowed slices
  (`name_parts`, pinned against `Path::file_stem`/`parent`), and reads
  text and mtime through one handle with the buffer sized up front.
- `vault_entity::frontmatter::has_type`: a `DeserializeSeed` visitor
  reads `type` and `tags` and skips every other value; a parity test runs
  it against the old `Mapping` reading over fourteen shapes of
  frontmatter, including sequences, scalars, broken YAML and non-string
  keys. The one intended difference: a block that repeats a key, which
  the `Mapping` refused, is read.
- `task::scan_vault` parses borrowed pages; `task::parse::parse_str` no
  longer copies its inputs.
- `just clippy-copies`.

Numbers: see the PR body (seeded scenario before/after, lint counts).

### PR 3: files / files-store / files-sync

Branch `perf/zero-copy-files` (STA-305). Fixes F1, F2, F3, F8 and F9.

- The chunk store, the version store and the sync seams hand out chunks
  and structural objects as `Bytes`: `ChunkStore::read_chunk`,
  `read_bytes` (was `read_to_vec`; one allocation sized from the
  manifest), `VersionStoreBackend::read_raw_object`,
  `FilesBackend::sync_object` / `sync_read_chunk`. Imports take
  `impl Into<Bytes>` or a slice, so neither a wire `Vec` nor a store's
  `Bytes` is copied on the way in.
- The vox wire is unchanged (`Vec<u8>`): at the `SyncService` boundary
  `Vec::from(Bytes)` takes over the uniquely owned buffer, no copy.
- `files_sync`'s commit cache holds `Bytes`, shared between the decode
  and the import threads; chunk batches are moved into their requests.
- `sync_has_object` stats the object instead of reading it.
- The `files` crate's 45 copy-lint hits are gone; `files`, `files-store`
  and `files-sync` deny the four lints.

Numbers (THEBATTLESHIP, canonical checkout under the build lease):

| | dev 319e410c | PR 3 |
|---|---|---|
| copy lints `files` / `files-store` / `files-sync` | 45 / 0 / 0 | 0 / 0 / 0 |
| memory_budget peak heap | 49.3 MiB | 49.3 MiB |
| memory_budget growth (12 rounds) | 0.0 MiB | 0.8 MiB |
| memory_budget RssAnon | 103832 KiB | 103368 KiB |

The seeded scenario hardly reads Files content (one File Root, a laptop
syncing small files), so its numbers don't move; the saving is per
chunk/object read and per pull, and it scales with file size. Growth is
noise around zero (1.4 MiB was measured at t_37712151). The budget is
tightened on the strength of the peak: heap 128 → 96 MiB, RssAnon
300 → 200 MiB; growth stays at 4 MiB.

### PR 2: task-server MCP / request path

Branch `perf/zero-copy-server` (STA-304, PR #223). Fixes S1–S4 and S7.

- `AppState::orgs` holds `Arc<OrgAppState>` and `AppState::org` hands
  out the `Arc`: a reference count per request instead of a copy of the
  struct. `iroh_host::bind_org` (and the tests that do the same) copies
  the org once to attach peering, then stores a fresh `Arc`.
- `/mcp` and `/org/{slug}/mcp` take the body as `Bytes` and parse it
  with `from_slice`. `parse_request` moves `id`, `method` and `params`
  out of the parsed object; `split_tool_call` moves `name` and
  `arguments` out of `params`. That replaces one deep clone of `params`
  and two of `arguments` per `tools/call`. A unit test pins both readers
  against the old `get` + `cloned` reading.
- JSON-RPC envelopes (`rpc_result`, `rpc_error`), tool schemas (`obj`)
  and the telemetry envelope are built by moving values into a map.
  `json!` serializes an interpolated value from a reference, so
  `json!({ "result": result })` deep-copied every tool answer once more.
- `TASK_MCP_TOKEN` is read once into `AppState::static_token`
  (`operator::StaticToken`, an `Arc<str>`). It used to be a fresh
  `String` from the environment on every MCP call and operator check.
  It's held on the `AppState`, not in a process static, so each test
  boot keeps its own token. The bearer itself is still cloned once per
  session lookup, because `architect_auth::CurrentSession` takes an
  owned `String`.
- `tool_catalog()` is built once (`OnceLock`) and returns
  `&'static [ToolDef]`.
- S7: the server's copy-lint hits are fixed (by-reference parameters,
  moves instead of clones, `#[derive(Clone, Copy)]` on a tag enum), and
  the crate denies the four lints.
- No on-disk or wire format change. One behaviour note: a body that
  isn't UTF-8 now gets JSON-RPC `-32700` instead of axum's plain-text
  400.

Numbers (THEBATTLESHIP, canonical checkout under the build lease, dev
e7af2a2f against the branch):

| | dev e7af2a2f | PR 2 |
|---|---|---|
| copy lints `task-server` (lib + tests + `it`) | 37 (23 `redundant_clone`, 14 `needless_pass_by_value`) | 0 |
| memory_budget peak heap | 47.2 MiB | 47.4 / 47.3 / 47.6 MiB |
| memory_budget growth (12 rounds) | 0.6 MiB | 0.0 / 0.0 / 1.1 MiB |
| memory_budget RssAnon | 104088 KiB | 106964 / 102692 / 100936 KiB |

(PR 2 column: three runs, on 98040bec, c04b529d and cd462a6c; the code
under test is the same in all three.)

The scenario's MCP traffic is a few small calls per org per round, so
the per-request savings don't show in its peak: the peak is set by boot
and the iroh/Files state (S5). Peak and RssAnon are flat within run to
run noise, and growth is noise around zero. The budget is tightened
anyway, since every run since PR 1 measured under 51 MiB peak: heap
96 → 80 MiB. Growth goes 4 → 3 MiB, about twice the worst run seen
(1.4 MiB). RssAnon stays at 200 MiB.

### PR 4: the vault snapshot cache

Branch `perf/zero-copy-vault-cache`. Fixes V12, V13, W1 (`list_pages`).

- `vault_live::snapshot`: `Vault::snapshot(root)` returns an
  `Arc<Vault>` from a process-wide cache keyed by root. Each call walks
  the root and `stat`s every page (length, mtime, and on Unix inode and
  ctime); a page whose stamp is unchanged is reused, any other is read
  again, and when nothing changed the previous `Arc` comes back as is.
  A page read within `RACY` (2 s) of its mtime is not trusted on its
  stamp (git's racily-clean rule), so a same-length rewrite in one
  timestamp tick is still seen. The vault watcher calls
  `snapshot::invalidate_path` for every file it reports. The cache keeps
  at most `MAX_CACHED_BYTES` (64 MiB) of page text, least recently used
  roots dropped first. No format changes: nothing on disk or on the wire
  moves.
- `VaultPage::raw` and `VaultBase::raw` are `Arc<str>`. Snapshots share
  unchanged pages' text, a cloned `Vault` no longer copies the vault,
  and `create_page` takes `impl Into<Arc<str>>`, so the entity stores'
  rename (`delete_page` + `create_page` of the same text) moves no text.
- On the snapshot: task, goal, milestone and workstream lists (and the
  workstream rollup's task and project scans, the milestone project and
  task lookups), the vault folder index, base views and full-text
  search (`vault_live::sync::Backend`), the file-version entity stores
  (a private copy of the snapshot, which now costs the path strings and
  not the text), and the wiki's `list_pages`.
- Scans borrow the snapshot: `looks_like_*_raw` + `parse_*` on
  `&page.raw` replace `into_proto()` + `parse_page`.

Numbers: see the PR body (memory_budget before/after, lint counts).

### PR 5a: vault sync moves, Bases rows, vault-doc

Branch `perf/zero-copy-long-tail` (STA-307). Fixes V14 and the copies
around B1; moves D1/D2 to architect.

- `vault_live::sync::move_file`: the `If-Match` check hashes the source
  through the 16 KiB stream (`sha256_file`), the move copies the file with
  `fs::copy` into the same temp-then-rename shape `write_file_atomic`
  has, and the ack's hash and the event's size are read from the file
  that landed. A refusal (stale `If-Match`, occupied destination) reads
  the file once, through `conflict`, as `put_file` already did. One
  visible difference: the copy keeps the source's permission bits,
  where the write made the file with the default mode.
- `vault_live::bases`: `BaseRow::from_frontmatter` takes the decoded map
  by value (`from_parts_full` parses its JSON and delegates);
  `execute_view_on` runs a view over borrowed rows (`execute_view`
  collects and delegates).
- `vault_live::sync::base_views` and `vault_obsidian::base_query` build
  rows from the map and run every view over one row set. The key order
  the rows saw is kept: serde_json is built without `preserve_order`, so
  the JSON string was always printed in sorted key order (the old
  comment in `row_of` said otherwise), and the map is still collected
  through a `serde_json::Map`. `frontmatter_map_matches_the_json_round_trip`
  and `rows_match_the_json_round_trip` pin both readings.
- D1/D2: the copies are in architect's `crdt` (registry fan-out,
  `Persistence::load_updates`); see the table.

Still open from PR 5's list (STA-309), held back because they share
crates with PRs 2–4: F4–F6 (files; PR 3 did F3), S6 (task-server), W2
(wiki-live), and the move of the four lints into
`[workspace.lints.clippy]`.

Numbers (THEBATTLESHIP, canonical checkout under the build lease, dev
319e410c against 7ce7ff82):

| | dev 319e410c | PR 5a |
|---|---|---|
| copy lints `vault-live` / `vault-obsidian` | 13 / 2 | 13 / 2 |
| memory_budget peak heap | 48.1 MiB | 51.0 MiB |
| memory_budget growth (12 rounds) | 0.5 MiB | 0.8 MiB |
| memory_budget RssAnon | 110636 KiB | 108344 KiB |

The seeded scenario makes no base view and no move, so it never runs
the changed code. The 2.9 MiB rise in peak isn't from this change; the
rebased tip is measured again in the PR body. Both runs are well under
the 80 MiB / 3 MiB budget, which this PR leaves as it is. The copy-lint hits in these two crates are
older than this change (none sit on the lines it touched). Clearing them
and adding the deny is STA-309.
