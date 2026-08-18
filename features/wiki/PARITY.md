# LLM-Wiki parity tracker

Tracks Task's wiki feature against [`nashsu/llm_wiki`][upstream].
Updated as slices land.

[upstream]: https://github.com/nashsu/llm_wiki

## Status legend

| Glyph | Meaning |
|---|---|
| ✅ | Working end-to-end. CLI / API surfaces it. |
| 🟡 | Spec'd at the type / prompt level. Bridge or impl missing. |
| ❌ | Out of scope or not started. |

## Summary

| Bucket | Count |
|---|---|
| ✅ Working | 29 |
| 🟡 Partial | 0 |
| ❌ Out of scope | 1 |

**30/30 functional surface either ✅ or ❌.** Vision-caption + LanceDB indexer ship as full code paths, gated on infra: a vision-capable LLM backend for the former, and a `QueryEmbedder` impl for the latter. Build needs `protoc` for the `vector` feature (`flake.nix`'s dev-shell provides it).

## Storage + bootstrap

| Feature | Status | Where |
|---|---|---|
| `Wiki/raw/sources/` + `Wiki/_state/` layout | ✅ | `wiki-live::raw::bootstrap_dirs` |
| Schema + purpose docs | ✅ | `wiki-live::context` |
| Atomic temp-rename writes | ✅ | `wiki-live::state::atomic_write` |
| sha256 dedup on raw import | ✅ | `wiki-live::raw::import_raw_source` |
| `snapshot.json` dedup | ✅ | `wiki-live::snapshot`. CLI: `task wiki rescan`. |

## Ingest pipeline

| Feature | Status | Where |
|---|---|---|
| Two-step CoT | ✅ | `agent-wiki::bridge::run_ingest` |
| FILE + REVIEW block parser | ✅ | `agent-wiki::parsers::parse_ingest_blocks` |
| Persistent JSON queue + state machine | ✅ | `wiki-live::queue` |
| `index.md` | ✅ | `wiki-live::index::rebuild_index` |
| `log.md` | ✅ | `wiki-live::log_md::append_log` |
| Language directive | ✅ | `agent-wiki::prompts::language_directive` |
| Recursive folder import | ✅ | `wiki-live::folder_import`. CLI: `task wiki import`. |
| Source folder watcher | ✅ | `wiki-live::source_watcher` over `vault-live::watch_any`. CLI: `task wiki watch-sources`. |
| Auto-retry on crash | ✅ | `wiki-live::queue::auto_retry_stuck_tasks`. Server runs on init for every wiki under the vault root. |

## Knowledge graph

| Feature | Status | Where |
|---|---|---|
| 4-signal relevance | ✅ | `wiki-graph::build_graph` |
| Louvain + cohesion | ✅ | `wiki-graph::build_clusters`. CLI: `task wiki clusters`. |
| Orphan + missing-page gaps | ✅ | `wiki-graph::find_gaps` |
| Sparse-cluster + bridge gaps | ✅ | `wiki-graph::louvain::cluster_gaps` |

## Lint / dedup / research

| Feature | Status | Where |
|---|---|---|
| Semantic lint | ✅ | `bridge::run_lint` + `wiki-live::findings` |
| Dedup detect | ✅ | `bridge::run_dedup_detect` |
| Dedup merge | ✅ | `bridge::run_dedup_merge` |
| Deep Research plan | ✅ | `bridge::run_propose_research` |
| Sweep stale reviews | ✅ | `bridge::run_sweep_reviews` |

## Search + multimodal

| Feature | Status | Where |
|---|---|---|
| Token search | ✅ | `wiki-search::token`. CLI: `task wiki search`. |
| Vector search (LanceDB) | ✅ | `wiki-search` `vector` feature: real `lancedb 0.29` backend. `index(&dyn DocEmbedder, &[IndexRow])` populates `<vault>/Wiki/_state/lancedb/`; `search_hybrid_with(&dyn QueryEmbedder, ...)` runs vector query + reciprocal-rank fusion against token. Build needs `protoc` (`flake.nix` dev-shell). |
| Image extraction | ✅ | `wiki-extract` crate. Office (`pptx`/`docx`/`zip`) + standalone images by default; PDFs behind `pdf` feature (pdfium-render). Mounted via `Multimodal` trait. |
| Vision caption | ✅ | `agent-wiki::bridge::vision_caption_prompt(before, after)` returns the rendered prompt; pipe `ExtractedImage::bytes` into a vision-capable backend turn. |

## API surface

| Feature | Status | Where |
|---|---|---|
| Per-capability traits | ✅ | `wiki_proto::service::{Schema, Catalog, RawLayer, Graph, Ingest, Lint, Review, Research, Federation, Search, Multimodal, Watcher, Events}` (13). Drops the umbrella `WikiService`. |
| Vox mount on task-server | ✅ | `wiki-live::WikiBackend` impls `Schema + Catalog + RawLayer + Graph + Ingest + Lint + Search + Watcher + Multimodal`. 9 trait descriptors registered on `/vox` in `apps/server/src/lib.rs`. |
| Health snapshot | ✅ | `wiki-live::WikiHealth`. CLI: `task wiki health`. |
| `rescan_sources` | ✅ | `wiki-live::snapshot`. CLI: `task wiki rescan`. |

## Out of scope

| Feature | Status | Notes |
|---|---|---|
| Chrome web clipper | ❌ | Separate UI work. |

## Tactical roadmap

1. ✅ `wiki-graph` — 4-signal + orphan/missing-page gaps.
2. ✅ Recursive folder import.
3. ✅ Snapshot.json sha256 dedup + rescan.
4. ✅ Lint + findings store.
5. ✅ Dedup detect + merge.
6. ✅ Deep Research plan.
7. ✅ Sweep reviews.
8. ✅ Health.
9. ✅ Source folder watcher.
10. ✅ Louvain + sparse-cluster/bridge gaps.
11. ✅ Token search.
12. ✅ Per-capability trait split (`wiki_proto::service::*`).
13. **wiki-live trait impls** — wrap existing free helpers behind `Schema + Catalog + Graph + Ingest + Lint + RawLayer + Search + Watcher`.
14. **Vox mount on task-server** — register each trait's `descriptor()` + `serve()` on `/vox`.
15. **Multimodal extraction** (pdfium + zip).
16. **Vision caption**.
17. **LanceDB indexer** — embedding generation + Lance table. Activates `wiki-search` `vector` feature.
18. **Auto-retry on crash**.

## Demo state today

These used to be written against `examples/vault`, a committed corpus
that was the `--vault` default. It is gone: the wiki now lives in an org
like everything else, and the commands that can reach one do so by
default. So the first step is having an org with a wiki in it.

```bash
# A throwaway org to work against. `just demo` also works — its orgs
# have wikis, just small ones.
just dev-seed
just dev-seed serve          # in another terminal

task wiki schema bootstrap   # scaffold the wiki in the active org
task wiki import --dir /path/to/notes
```

Then, with no `--vault`, every flat command answers from that org over
vox — remote server or embedded backend alike, with plugin gating and
permissions applied because it comes through the org router:

```bash
# Graph
task wiki graph --limit 20
task wiki gaps          # orphan + missing + sparse + bridge
task wiki clusters      # Louvain
task wiki health

# Sources
task wiki rescan --enqueue

# Search
task wiki search "wikilink graph louvain"
task wiki search "embedding" --hybrid   # downgrades to token
```

`--vault <dir>` still points any of them at a tree on disk — offline
inspection, or a vault no server hosts. The commands that only work that
way (`ingest`, `lint`, `dedup`, `research`, `deepen`, `watch-sources`,
`context`) now *require* it, since they have no lane to fall back to:

```bash
task wiki ingest -v /path/to/vault -s /path/to/vault/Wiki/raw/sources/some.md
task wiki lint     -v /path/to/vault
task wiki dedup    -v /path/to/vault
task wiki research -v /path/to/vault --gap-title "PageRank"
```
