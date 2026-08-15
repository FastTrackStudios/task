# wiki feature

**Status:** partially shipped — needs triage (2026-07-27). `wiki-{proto,archive,extract,graph,live,search}` all exist under `features/task/wiki/`; per-slice completion was not verified.

Task's port of [Karpathy's LLM-Wiki pattern][karpathy-wiki],
modeled on [`nashsu/llm_wiki`][llm-wiki]. A vault's `Wiki/`
folder becomes an LLM-maintained knowledge base — the agent
ingests sources, builds pages with cross-references,
maintains a content catalog + chronological log, surfaces
knowledge gaps, and accepts curator review/research actions.

The file-storage layer is already done — this is
[`vault`](vault-feature.md). The wiki feature adds **structure
+ workflow on top** of plain markdown.

[karpathy-wiki]: https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f
[llm-wiki]: https://github.com/nashsu/llm_wiki

## Crate plan

```text
features/wiki/
├── wiki-proto/   ✅ shipped — wire contract (architect::rpc trait)
├── wiki-live/    ⏳ file-backed primary backend
├── wiki-agent/   ⏳ LLM-glue boundary (claude CLI / Anthropic / Hermes)
├── wiki/         ⏳ facade re-export
└── wiki-ui/      ⏳ Dioxus components (graph view, review queue, lint findings)
```

`wiki-proto` is the spec. Everything else stacks on it. Same
shape as `vault` / `vault-live` / `vault-proto`.

## Shipped: `wiki-proto`

Wire contract for the whole feature. Single
`#[architect::rpc] trait WikiService` covering bootstrap,
schema/purpose, index/log, graph, ingest, lint, review,
research, federation, and live events. No backend — backends
implement the trait and mount via `wiki_proto::serve(state)`
the same way `vault_proto::serve(backend)` does today.

Module map (`features/wiki/wiki-proto/src/`):

| File              | What                                            |
|-------------------|-------------------------------------------------|
| `service.rs`      | `WikiService` trait + architect-emitted client. |
| `paths.rs`        | Canonical paths inside `Wiki/`.                 |
| `schema.rs`       | `SchemaDoc`, `PurposeDoc`, defaults.            |
| `log.rs`          | `WikiIndex`, `IndexEntry`, `LogEntry`, `LogOp`. |
| `graph.rs`        | 4-signal relevance, `Cluster`, `KnowledgeGap`.  |
| `raw.rs`          | `ImportRawSource`, `RawSourceRef` — the immutable layer. |
| `ingest.rs`       | `IngestTask`, `IngestStatus`, `AnalysisDraft`, `PageDraft`. |
| `lint.rs`         | `LintFinding`, `LintScope`, `FindingAction`.    |
| `review.rs`       | `ReviewItem`, `ReviewKind`, `ReviewAction`.     |
| `research.rs`     | `ResearchPlan`, `RawSource`.                    |
| `search.rs`       | `SearchOpts`, `SearchHits`, `SearchMode` — token / hybrid. |
| `health.rs`       | `WikiHealth` snapshot.                           |
| `multimodal.rs`   | `ExtractOpts`, `ExtractedImage` — Phase-1 image extraction. |
| `federation.rs`   | `PeerWiki`, `PeerPullResult`, `CrossWikiPageRef`. |
| `event.rs`        | `WikiEvent` for live subscribers.               |
| `error.rs`        | `WikiError`.                                    |

## The raw vs. articles split

`Wiki/raw/sources/` is the **immutable** layer (mirrors llm_wiki's
`raw/sources/`). Bytes only land via
`import_raw_source` or `submit_research_result`; they only
leave via `delete_raw_source`. The agent reads from
`sources/` and cites entries via the `sources:` frontmatter
field, but never rewrites the file bytes — backends enforce
this by rejecting `record_pages` drafts targeting paths under
`Wiki/raw/sources/`.

Everything else under `Wiki/` is **mutable**:
`<Type>/<Slug>.md` pages are LLM-authored, `index.md` +
`log.md` are LLM-maintained, `media/` holds extracted +
captioned images, `_state/` holds opaque agent state.

## On-disk layout

Everything lives at `<vault>/Wiki/`. Plain markdown for
curator-visible state, JSON for opaque agent state.

```text
<vault>/Wiki/
├── schema.md          ← contract (per-vault, agent reads on every call)
├── purpose.md         ← goals / scope / key questions
├── index.md           ← content catalog, LLM-maintained
├── log.md             ← append-only operation timeline
├── overview.md        ← optional global synthesis
├── raw/
│   └── sources/       ← imported raw documents (immutable)
├── media/             ← extracted images, attachments
├── _state/            ← opaque agent state (JSON)
│   ├── ingest_queue.json
│   ├── review.json
│   ├── lint_findings.json
│   ├── research_plans.json
│   ├── peers.json
│   └── snapshot.json
└── <Type>/<Slug>.md   ← actual wiki pages (Concepts/, Entities/, ...)
```

`_state/` is opaque to humans by convention (and visually
sorts below content). Backends can swap formats freely; the
trait surface is the only stable contract.

## The 4-signal graph (llm_wiki defaults)

| Signal          | Weight | What it captures                          |
|-----------------|--------|-------------------------------------------|
| Direct link     | ×3.0   | `[[wikilink]]` from A → B                 |
| Source overlap  | ×4.0   | A and B both cite the same source         |
| Adamic-Adar     | ×1.5   | Σ 1/log(deg(n)) over shared neighbors     |
| Type affinity   | ×1.0   | Both have the same `type:` frontmatter    |

Same weights as `llm_wiki/src-tauri/src/api_server.rs::handle_graph`.
Overridable via `GraphOpts::weights`.

## Two-step ingest, agent-agnostic

The trait deliberately keeps the LLM out of band. An agent
(Claude CLI, Anthropic API, Hermes) drives the pipeline by:

1. `enqueue_ingest(source_path)` — register a new raw file.
2. `claim_next_ingest()` — pop one `Pending` task.
3. Agent reads source + `schema.md` + `purpose.md` + `index.md`.
4. `record_analysis(task_id, analysis)` — store step-1 CoT.
5. Agent generates pages.
6. `record_pages(task_id, pages)` — backend writes atomically,
   updates index, appends to log.
7. `complete_ingest(task_id)` — done.

State machine enforced by the backend; out-of-order calls
return `WikiError::IllegalState`.

## Slices

### 1. ✅ Proto (this commit)
`wiki-proto` builds clean, exports the trait + types.

### 2. `wiki-live` (file-backed backend)

- `bootstrap` — write defaults, scaffold dirs.
- `read_schema` / `read_purpose` / `write_*` — plain file IO.
- `read_index` / `rebuild_index` — scan vault, render markdown.
- `append_log` — open-append with header serializer.
- `enqueue_ingest` / `claim_next_ingest` / etc. — queue
  persisted as `_state/ingest_queue.json`, mutex-locked per
  wiki, crash-safe atomic writes (rename-over-temp).
- `lint` (scope: Orphan, Stale, MissingPage, Schema) — no LLM
  needed for these. Persist findings.

Defer: `lint(Contradiction / MissingCrossRef)`, `gaps`,
graph/clusters — those slot into slice 3.

### 3. `wiki-graph` (pure computation)

- 4-signal edge scorer.
- Louvain via `graphology-communities-louvain` (rust port or
  vendor). Cohesion score.
- `gaps` over the graph: orphans, sparse clusters, bridge
  nodes (high betweenness), missing-page detector (mentions
  without targets).
- `build_graph` returns the snapshot; caching is the
  backend's call (invalidate on `PageWritten`).

### 4. `wiki-agent` (LLM glue boundary)

- Trait: `LlmBackend { analyze_source, generate_pages,
  draft_query_answer, ... }`. One method per agent-driven
  step in the trait.
- Impls: `ClaudeCli` (shells out, same as llm_wiki's
  `claude_cli.rs`), `AnthropicApi` (HTTP), `Hermes` (calls our
  agent).
- The `task-cli` `wiki ingest` subcommand wires a chosen
  backend to a wiki client and drives the queue.

No code in this slice yet — the trait surface is the
extension point. Build it when the first LLM call is needed.

### 5. `wiki-cli` (`task wiki ...` subcommands)

Mirrors `task list` / `task capture`. Commands the curator
will actually use:

- `task wiki bootstrap [--vault PATH]`
- `task wiki ingest <source> [--vault PATH]`
- `task wiki list-queue`
- `task wiki graph [--type concept] [--limit N]`
- `task wiki gaps`
- `task wiki lint [--scope orphan|stale|...]`
- `task wiki review` (interactive)
- `task wiki research <gap-id>`
- `task wiki peers` (add / list / pull)

### 6. `wiki-ui` (Dioxus)

- Graph view (sigma.js-equivalent for Dioxus; or render to SVG).
- Lint findings panel.
- Review queue with suggestion picker.
- Ingest queue progress.
- Schema/purpose editor.

### 7. Federation (down the road)

Multi-vault wikis. `pull_from_peer`, cross-wiki wikilinks
(`[[alice:Page]]`), divergence → review queue. Stays optional —
a single-vault wiki ignores all federation surface.

### 8. Multimodal (down the road)

Phase 1 of llm_wiki's `multimodal-images.md`: PDF + Office
image extraction (pdfium for PDFs, zip+XML for PPTX/DOCX),
factual captions via vision LLM, dedup by SHA-256, file
under `Wiki/media/<source-slug>/`. The trait already accepts
`RawSource { bytes, mime, ... }` — multimodal slots in without
schema changes.

## Open questions

- **Index format** — markdown table vs. nested lists?
  llm_wiki uses categorized lists. Probably copy.
- **Log header date** — local wall-clock or UTC? llm_wiki uses
  local. Easier to grep with local; UTC is portable. Lean local
  since these are personal vaults.
- **Sources mirror** — `Wiki/raw/sources/` mirrors disk; do we also
  hash + dedupe so the same PDF imported twice doesn't bloat?
  Yes, via the `snapshot.json` mechanism (sha256 keyed).
- **Federation transport** — reuse `vault-sync`'s WebSocket
  channel, or a separate HTTP-only `wiki-sync`? Probably the
  latter — federation is read-mostly, doesn't need the
  byte-level WS push.
- **What about non-Wiki pages?** Tasks, Daily, Meetings, etc.
  are in the same vault but outside `Wiki/`. The graph could
  include them or not. Default: scope to `Wiki/` only; offer
  an opt-in to widen.

## Why this and not something simpler

A `task wiki ingest` that just shells out to Claude and writes
files would work for a single user. The trait surface buys:

- **Reproducibility** — the agent drives the same surface a
  human + CLI does. No special "agent mode".
- **Federation** — peer wikis call the same trait remotely.
- **UI** — Dioxus components consume the trait, no parallel API.
- **Testability** — the backend's state machine is pure logic;
  unit tests cover queue transitions without an LLM.

Same argument we made for `vault-proto`. Worth the upfront
typing.
