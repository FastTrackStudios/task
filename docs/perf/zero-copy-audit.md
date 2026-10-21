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
| V12 | `Vault::open` per list call | re-reads every page of the vault on every list | **vault × list calls**: the largest remaining churn in this area | **later**: cache the snapshot per org, invalidated by the watcher (needs its own PR; changes no format) |
| V13 | `VaultPage` / `VaultBase` derive `Clone`, `Vault` derives `Clone` | a cloned `Vault` copies every page | unknown; callers not yet audited | **later**: find the callers under `redundant_clone`, then hold pages as `Arc<str>` if a cache (V12) shares them |
| V14 | `vault_live::sync` `put_file` / `move` paths | `fs::read` of source and destination to compare and hash | file size × writes | **later**: stream-compare (equal length, then 16 KiB chunks) |
| V15 | `lexorank` | `String::from_utf8(bytes.to_vec())` | tiny | **leave** |

### task-server (MCP, sharing, request plumbing)

| # | Site | Copy | Bytes × frequency | Status |
|---|------|------|-------------------|--------|
| S1 | `AppState::org(slug)` | clones the whole `OrgAppState` (strings, roots, plugin set, auth, sharing handles) out of the map on every request | struct × every MCP / HTTP call (`authenticate_for` 682 k allocs) | **next**: store `Arc<OrgAppState>`, hand out the `Arc` |
| S2 | `mcp::mcp_dispatch` | body `String` parsed to a `serde_json::Value`, then `params` and `id` deep-cloned out of it | request size × 2 × calls (1.69 M allocs) | **next**: take `params`/`id` out of the owned map (`remove`), parse from the body bytes |
| S3 | `mcp::authenticate_for` | `std::env::var("TASK_MCP_TOKEN")` (a `String`) per call; token cloned per session lookup | per call | **next**: read the static token once at startup; pass the token by reference where architect allows |
| S4 | `mcp::call_tool`, `tool_ok`, `rpc_result` | tool results built as `Value` then serialized; the catalogue rebuilt per `tools/list` | 388 k + 73 k + 105 k allocs | **next**: build `tool_catalog` once (`OnceLock`) and serialize straight to the response |
| S5 | `iroh_host::bind_org` | 22.7 MiB of the 67 MiB peak | retained, not churn | **leave** here: iroh endpoint and blob store state, owned by t_4219725c / t_911268fa |
| S6 | `sharing::{sources, subtree}` | path strings rebuilt per entry while walking a shared subtree | entries × share listings | **later** |
| S7 | 36 copy-lint hits (server) | mostly `needless_pass_by_value` on handlers' `String`/`Vec` args | small each | **next**, with the deny |

### files / files-store / files-sync

The Files feature moves user files, so its copies scale with file size
rather than with request count — fewer calls, far more bytes each.

| # | Site | Copy | Bytes × frequency | Status |
|---|------|------|-------------------|--------|
| F1 | `ChunkStore::read_to_vec`, `Backend::sync_object`, `sync_read_chunk` | whole file / object / chunk into a `Vec<u8>` | file size × reads | **next**: return `Bytes` from the store (chunks are already immutable, content-addressed); stream objects larger than a chunk |
| F2 | `files_sync` object cache (`HashMap<String, Vec<u8>>`) | each object held once per lane that fetched it | object size × lanes | **next**: `Bytes` values, shared across lanes |
| F3 | `files_sync` `chunks(root_id, batch.to_vec())` | batch of hashes copied per request | small × batches | **later**: take the batch by value |
| F4 | `files::lane::federation::trim_to_byte_range` | full object read, then trimmed | object size × ranged reads | **later**: read only the range (seek + `take`) |
| F5 | `files::{durable, hydration, ignore}` `fs::read` | whole file to inspect it | file size × calls | **later**: stream where only a hash/prefix is needed |
| F6 | `files::backend` merge (`side_a.to_vec(), side_b.to_vec()`) | both sides copied to merge | file size × merges | **later**: merge from slices |
| F7 | `FileId::new(id.as_bytes().to_vec())` (content.rs) | 32-byte ids | tiny | **leave** |
| F8 | 45 copy-lint hits (files) | the largest lint cluster | small each | **next**, with the deny |

### vault-doc (CRDT peer)

| # | Site | Copy | Bytes × frequency | Status |
|---|------|------|-------------------|--------|
| D1 | `peer`/`replica` update channels (`vox::Rx<Vec<u8>>`, `UnboundedReceiver<Vec<u8>>`) | each update fanned out by cloning its `Vec` | update size × subscribers | **later**: `Bytes` in memory; the wire stays `Vec<u8>` (vox encoding unchanged) |
| D2 | `load_updates` → `Vec<Vec<u8>>` | every stored update for a doc loaded at once | doc history × opens | **later**: stream updates into the doc |

### wiki-live

| # | Site | Copy | Bytes × frequency | Status |
|---|------|------|-------------------|--------|
| W1 | 66 `fs::read`/`read_to_string` sites | whole pages read per operation | page × calls | **later**: reuse the vault snapshot (V12) instead of re-reading |
| W2 | 105 `to_owned` / 143 `clone` | mostly path and title strings | small each | **later**, with the lint |

### Bases (`vault_live::bases`, `vault_obsidian::base_query`)

| # | Site | Copy | Bytes × frequency | Status |
|---|------|------|-------------------|--------|
| B1 | query execution | evaluates over owned `Value`s of every page's frontmatter | vault × base views | **later**: evaluate over borrowed frontmatter (`#[serde(borrow)]` into `Cow<str>`) |

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
