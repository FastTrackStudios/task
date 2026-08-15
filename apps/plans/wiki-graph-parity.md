# wiki-graph — feature parity with LLM-Wiki + Graphify

**Status:** parity tracker — ongoing. Living scoreboard, never "done".

**Target:** make `wiki-graph` feature-complete with both nashsu/llm_wiki and safishamsi/graphify, but for the **entire wiki** (code + markdown + mixed), not just code projects.

## What we have today

`features/wiki/wiki-graph/` already does the markdown side:

- `scan.rs`  — walk `Wiki/` for `.md` pages
- `parse.rs` — extract `[[wikilinks]]` from page bodies
- `build.rs` — `build_graph(vault_root, opts)` produces a typed `WikiGraph`
- `louvain.rs` — community detection
- `scoring.rs` — 4-signal relevance
- `gaps.rs`  — orphans, missing-page detection
- `tier_lint.rs` — layered-access rules (Knowledge / LLM / Vault)

## Gap audit vs reference projects

| Feature | LLM-Wiki | Graphify | Us |
|---|---|---|---|
| Markdown wikilink graph | ✓ | — | ✓ |
| Community / cluster detection | ✓ | ✓ | ✓ (Louvain) |
| Relevance scoring | ✓ | ✓ | ✓ (4-signal) |
| Gap detection (orphans, missing) | ✓ | — | ✓ |
| **Tree-sitter code extraction** | — | ✓ (14 langs) | **missing** |
| **Confidence labels (EXTRACTED / INFERRED / AMBIGUOUS)** | — | ✓ | **missing** |
| **Call-graph (2nd-pass INFERRED edges)** | — | ✓ | **missing** |
| **God-node / centrality analysis** | — | ✓ | partial (scoring is similar) |
| **Surprises / unexpected connections** | — | ✓ | **missing** |
| **LLM-suggested questions in GRAPH_REPORT** | — | ✓ | **missing** |
| **Affected subgraph (for a diff)** | — | ✓ | **missing** |
| **Watch mode (re-extract on change)** | ✓ | ✓ | partial (wiki has watcher; not graph-extraction) |
| **Semantic cache (SHA-skip)** | ✓ | ✓ | partial (raw import dedupe) |
| **Interactive HTML viewer** | ✓ | ✓ | **missing** |
| **MCP / RPC server** | — | ✓ | partial (`task wiki graph` over vox) |
| **Skill / runbook** | ✓ | ✓ | partial (`llm-wiki`; no code-graph skill) |
| **Context subgraph for LLM (token-budget aware)** | partial | implicit | **missing** ← biggest agent value |
| **PR / diff analysis** | — | ✓ | **missing** |

## Design — unified graph

The wiki-graph engine accepts **any source**:

```
Node {
  id: String,                    // stable, content-derived
  label: String,                 // human display
  kind: NodeKind,                // Page | Symbol | Source | Person | Concept | …
  source_kind: SourceKind,       // Markdown | Code | Json | …
  source_file: PathBuf,
  source_location: Option<String>,  // L42 / L42-50 / fn:name / class:name
  language: Option<String>,      // "rust", "ts", "python" — None for prose
  cluster: Option<String>,
  centrality: Option<f64>,
}

Edge {
  source: NodeId,
  target: NodeId,
  relation: Relation,            // Wikilink | Call | Import | Uses | Mentions | Implements | …
  confidence: Confidence,        // Extracted | Inferred | Ambiguous
  weight: f64,
}
```

Existing markdown pages are `NodeKind::Page` with `SourceKind::Markdown`. New code symbols are `NodeKind::Symbol` with `SourceKind::Code`. Cross-edges happen naturally: a wiki page that mentions `Vec::push` produces a `Wikilink` edge to the `vec::Vec::push` symbol node.

## Roadmap (4 PRs)

### PR 1 — Foundation: code extraction (Rust + TS first)

- New module `wiki-graph/src/code_extract.rs`
- Use `arborium-tree-sitter` (already a workspace dep with Rust + TS/JS/Python support)
- Extractor per language: functions, structs, traits, impls, imports → nodes/edges with EXTRACTED confidence
- Call-graph 2nd pass: walk function bodies, match identifiers to known symbols → INFERRED edges
- Extend `WikiGraph` types in `wiki-proto::graph` with `kind`/`confidence`/`language`
- CLI: `task wiki graph --include-code` flag (existing verb gains the code path)
- Tests: extract Rust + TS fixtures, assert node + edge counts

### PR 2 — Analysis: god-nodes, surprises, suggested questions

- New `wiki-graph/src/analyze.rs`
- God-nodes: top-N by degree centrality + betweenness
- Surprises: edges between clusters that share few intermediaries (gephi-style "bridges")
- LLM-augmented `task wiki graph report` → `GRAPH_REPORT.md` with: top god-nodes, surprising connections, suggested questions, gaps. The LLM portion goes through `agent-wiki::bridge`
- Tests over fixed fixture graphs

### PR 3 — Context for LLMs (the headline agent value-add)

- New `wiki-graph/src/context.rs` — `subgraph_for_query(query, budget_tokens)` returns a relevance-ranked subgraph that fits a token budget
- Two query forms: by node id (`Vec::push`) or by free text (LLM-scored)
- New CLI: `task wiki graph context <query> --budget 8000` prints a markdown summary suitable for stuffing into an LLM prompt
- Replaces "read raw files" — agents get a structural view they can navigate
- Bench: corpus-tokens vs subgraph-tokens (graphify's benchmark module)

### PR 4 — Affected / diff + HTML viewer + skill

- `task wiki graph affected <file>` — subgraph touched by a file change
- `task wiki graph affected --diff <commit>..` — for PR review
- Interactive HTML viewer (vanilla + D3 or vis-network) at `graph.html`
- Watch mode: re-extract changed files (incremental SHA cache)
- Skill: `~/.claude/skills/wiki-graph/SKILL.md` walking an agent through the loop

## Open design questions

- **Symbol identity across languages**: how to fingerprint `Foo::bar` in Rust vs `foo.bar` in TS. Probably: `<lang>:<module-path>:<symbol>`. Cross-language edges (e.g. JS calls a Rust wasm export) are AMBIGUOUS.
- **Storage**: do we persist the graph to disk (`graph.json` like graphify) or rebuild on demand? Build-on-demand for small projects; cached `graph.json` updated by watcher for large ones.
- **Index of the wiki itself as part of the graph**: every wiki page already has a `sources:` array. Edges from page → source make the wiki itself navigable through the same graph.

## Non-goals (for now)

- 14-language extraction parity with graphify. Start with Rust + TS + Python (the langs Task touches).
- MCP server bindings — `task wiki graph` over architect-rpc already covers the wire surface.
- PR-level GitHub integration. The local `affected --diff` is enough until federation makes remote PRs first-class.
