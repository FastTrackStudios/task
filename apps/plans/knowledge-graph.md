# Knowledge graph slice — design notes

**Status:** research — folded into `wiki-feature.md` slice 3. Kept for the algorithm detail, not as a work item.

Research-only doc. Distills what to crib from `nashsu/llm_wiki`
(cloned at `~/Development/research/llm_wiki/`) for Task's graph
view, plus our own constraints.

## The llm_wiki model (verified by reading their code)

Files of interest:

- `src/lib/wiki-graph.ts` (304 LOC) — node + edge construction,
  Louvain community detection, cohesion calc.
- `src/lib/graph-relevance.ts` (312 LOC) — the 4-signal scorer
  (`calculateRelevance(nodeA, nodeB, graph)`).
- `src/lib/graph-filters.ts`, `graph-visibility.ts`,
  `graph-insights.ts`, `graph-search.ts` — UI filters,
  hide/show, surprising-connection insights, semantic search.
- `src/components/graph/graph-view.tsx` (1239 LOC) — Sigma +
  React rendering, force-atlas layout, interactions.

Stack: `sigma` (WebGL renderer) + `graphology` (data
structure) + `graphology-communities-louvain` +
`graphology-layout-forceatlas2`.

### The four signals (with weights)

| Signal              | Weight | Source            |
|---------------------|--------|-------------------|
| Direct links        | 3.0    | `[[Wikilink]]` counts |
| Source overlap      | 4.0    | shared `sources:` frontmatter |
| Adamic-Adar         | 1.5    | shared neighbors, discounted by `1/log(degree)` |
| Type affinity       | 1.0    | 5×5 matrix on `type:` (concept/entity/source/synthesis/query) |

`relevance(A, B) = directLink + sourceOverlap + adamicAdar + typeAffinity`.

### Type-affinity matrix (from `graph-relevance.ts`)

```
            concept  entity  source  synthesis  query
concept       0.8     1.2     1.0      1.2       1.0
entity        1.2     0.8     1.0      1.0       0.8
source        1.0     1.0     0.5      1.0       0.8
synthesis     1.2     1.0     1.0      0.8       1.0
query         1.0     0.8     0.8      1.0       0.5
```

Higher = more relevant. Note the **self-type discount** (same
type is less interesting); concept↔synthesis at 1.2 captures
"bridging" pairs.

### Louvain + cohesion

After scoring every pair, build a weighted undirected graph and
run Louvain. Per-community **cohesion** =
`intra_community_edges / possible_edges`. Sort communities by
node count; renumber sequentially.

This becomes the visual cluster: same color → same community.

### Graph insights (the smart layer)

- **Surprising connections**: high relevance between nodes in
  *different* communities. "These two clusters share more than
  expected."
- **Knowledge gaps**: nodes with high cohesion to a community
  they're not in — should they be linked there?

## Rust translation — what we'd build

### Crate layout

```
features/knowledge/knowledge-graph/      [new]
  src/
    lib.rs           — public surface
    relevance.rs     — the 4-signal scorer, ported as pure fns
    louvain.rs       — community detection (either crib
                        graphology-communities-louvain in Rust
                        or pull `petgraph-louvain` if it exists,
                        else write ~200 LOC)
    layout.rs        — force-directed layout (port forceatlas2
                        or use `fdg` crate)
    insights.rs      — surprising connections + gaps
    types.rs         — Node, Edge, Graph, Community structs
```

`knowledge-graph` is wasm-clean (pure compute, no IO). The
`vault::Vault` snapshot feeds it; UI consumes the output.

### Wire trait (optional)

For now the graph is computed *client-side* from a `vault::Vault`
snapshot, so no new wire trait is needed. If we later want the
server to pre-compute on heavy vaults (8k-page Observatory),
add `VaultGraph` next to `VaultSync`:

```rust
#[architect::rpc]
pub trait VaultGraph {
    fn snapshot(&self, vault_id: &str) -> Result<GraphSnapshot, GraphError>;
    async fn subscribe(&self, vault_id: String, tx: Tx<GraphDelta>);
}
```

Defer until we have a real perf need.

### UI

`features/knowledge/knowledge-ui` already has a `graph_*` module
scaffold (`graph_loader.rs`, `graph_writer.rs`). Today they
read/write the Loro-backed knowledge model. The new graph view
sits next to them.

Renderer pick:
- **SVG + Dioxus**: easy, no extra deps, OK for <500 nodes.
- **Canvas2D**: medium effort, handles 2-5k nodes.
- **WebGL via `wasm-bindgen`** (Sigma-equivalent in Rust): nothing
  great off-the-shelf; would require writing the renderer.

For v1: **SVG**. Our example vault has 79 nodes; even the
Observatory at 8k is borderline-SVG-feasible if we cluster
aggressively + cull off-screen.

## Slice plan (when the user says "go")

1. **`knowledge-graph` crate** with the pure compute surface
   (relevance + Louvain + force-atlas + insights).
   Ported tests from llm_wiki's `*.test.ts`. ~600 LOC.

2. **CLI command**: `task-cli graph examples/vault` — dumps
   `{nodes, edges, communities}` as JSON. Lets us iterate on
   the model without UI.

3. **Dioxus `<GraphView/>` component** in
   `features/knowledge/knowledge-ui`. Takes a
   `knowledge_graph::Graph` + selection signal; renders SVG
   with click-to-focus + community-color fill.

4. **Wire into `editor::EditorApp`** as a side-panel or
   modal. Toggle via slash command.

## Sanity-check fixture

`examples/vault/` now ships 79 pages with explicit
`type:` + `sources:` frontmatter on the concept / project / synthesis
pages — exactly the shape llm_wiki's scorer needs. Run the
relevance + Louvain port against it as the first integration
test.

## What we deliberately don't crib

- **Vector semantic search** (LanceDB + OpenAI embeddings) —
  out of scope; Task's editor + manual `[[wikilinks]]` are
  the primary search path. Revisit if it becomes a real gap.
- **Deep Research / web search auto-ingest** — LLM Wiki's whole
  raison d'être, but explicitly opposite to Task's "you write
  it, we sync it" model. Not coming back.
- **Chrome web clipper** — same reason. The vault is whatever
  the user puts in `~/Documents/Vault`.
