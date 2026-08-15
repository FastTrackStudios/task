# Knowledge Primitives — typed links · confidence · visibility · publishing

> Status: **design proposal** (2026-06-16), from the research in this session. Generalizes
> beyond the Bible — these are primitives for *any* topic. Grounded in what the repo
> already has (see "Existing hooks"). The Bible is the first heavy user.

## The goal (from the user)

- Link verses ↔ verses, verses ↔ wiki entries (people, places), notes ↔ anything.
- Tag verses topically ("where does the Bible talk about money / pain").
- A **private journal** in the vault that links to everything, kept private.
- **Publish** just the *interconnections* — a shareable graph of verses ↔ ideas ↔ wiki
  entries — **without** publishing private vault content.
- **Quality filters**: separate unstructured thoughts / feelings / opinions from
  facts / research / strongly-established links. Show a graph of only the solid links.

## The model (research-backed — see §sources)

Two first-class things: **nodes** and **typed links**. The field has converged on
**three orthogonal axes** for nodes (Gwern): don't conflate "how finished" with "how
sure" with "how important."

### Node (a vault note, wiki page, verse, entity, …)
| field | values | borrowed from |
|---|---|---|
| `maturity` | `seedling` → `budding` → `evergreen` | Maggie Appleton digital-garden growth stages |
| `confidence` | `certain > likely > possible > unlikely > speculative` (ordinal) | Gwern / Kesselman estimative words |
| `visibility` | `private` (default) → `unlisted` → `public` | Quartz ExplicitPublish (opt-in is the safe default) |
| `importance` | `0–10` (optional) | Gwern |
| `last_reviewed` | date | Matuschak (staleness is a liability when published) |

This is exactly the user's "thoughts/feelings/opinions vs facts/proven": `confidence` +
`maturity` carry it, and `visibility` gates publishing.

### Typed link (a FIRST-CLASS object, not just an edge — the key insight)
Nanopublication / RDF-star lesson: a link carries its *own* confidence, visibility, and
provenance, so you can filter and publish at link granularity.
```
{ source, target, relation, confidence, visibility, provenance{ source_ref?, created_by, created_at, derived } }
```
**Relation vocabulary** (directional, single-token, named inverses, extensible):
- topic (SKOS): `broader`⇄`narrower`, `related`
- structure (Breadcrumbs): `up`⇄`down`, `next`⇄`prev`
- definitional: `defines`, `instance-of`⇄`has-instance`, `example-of`
- epistemic (argument mapping): `supports`⇄`supported-by`, `refutes`⇄`refuted-by`, `cites`/`source-for`
- scripture-specific: `cross-ref` (verse↔verse), `fulfills`/`quotes`, `mentions` (verse→entity), `tagged` (verse→topic)

### Publishing
- **Opt-in** (private by default). Publishing exports nodes with `visibility ≥ unlisted`
  **plus their links** whose `visibility` allows it.
- **Public → private link policy** (declared at publish time): `redact` (strip href, keep
  text — default, avoids leaking private titles) | `drop` | `stub`. Lint warns on any
  public node linking a private target.
- The published artifact is a **graph of interconnections** (verse↔verse, verse↔entity,
  idea↔idea) filtered by confidence/maturity — exactly the user's ask.

### Reader-facing quality filters (the differentiator — nobody ships this)
A confidence slider ("only ≥ likely"), maturity badges, relation-type toggles ("only
typed/epistemic edges, hide loose wikilinks"). The data model above is what enables it.

## Existing hooks in this repo (lift, don't reinvent)

The local survey found the model already exists for *code* — the job is to raise it to
notes/verses and add visibility:
- **`features/wiki/wiki-graph/src/code_extract.rs`** already has `Relation {Defines,Calls,
  Imports,Implements}` + `Confidence {Extracted,Inferred,Ambiguous}` per edge. Generalize
  these enums.
- **`features/wiki/wiki-proto/src/graph.rs`** `GraphEdge`/`GraphNode` — add `relation` +
  `confidence` fields.
- **`features/vault/vault-live/src/property_schema.rs`** `PropertyType::EnumWithMetadata`
  — model `confidence`/`visibility`/`maturity` as page properties (with colors/icons).
- **`features/view/view-knowledge-graph/src/filters.rs`** `GraphFilterState` already prunes
  by kind/node — add confidence/relation/visibility filters here.
- **`features/wiki/wiki-graph`** 4-signal relevance scorer + Louvain communities — built;
  feeds the published graph.
- **Federation** (`wiki-proto/federation.rs`, `plans/federated-task-platform.md`) +
  `plans/done/vault-publisher.md` (Quartz-style static export) — the publish
  hooks. Add the visibility filter to the export path.
- **Layered model** already enforced: `vault/` → `wiki/Knowledge/` (curated, self-contained
  link-in target) → `resources/`. Maps onto private-journal → publishable-facts.

## Bible-specific data to bundle (all CC BY / PD — research §1)

These populate the link/tag graph with authoritative data (the resource-library pattern):
- **Cross-references** → OpenBible.info `cross-references.txt` (CC BY, ~340k verse↔verse,
  **signed votes** — negative = bad link, so confidence falls out of the data). Bundle to
  `<org>/resources/crossref/`. Optional: raw TSK SWORD for phrase anchors.
- **Topical tags** → OpenBible.info `topic-votes.txt` + `topic-scores.txt` (CC BY, **weighted**)
  as primary; Nave's Topical (BradyStephenson CSV, CC BY) for a PD taxonomy. → `resources/topics/`.
- **Entities** (people/places → verses, genealogy, geo) → STEPBible **TIPNR** (CC BY,
  stable `uStrong` ids). → `resources/entities/`. Seeds wiki entity pages
  (`mentions` links from verses).
- Join key: a single canonical verse id (we have `VerseId` OSIS + BBCCCVVV) — normalize
  all sources to it. The vote/score columns become link `confidence`.

## Build order

> User chose **generalized primitives first**, then pour Bible data in.

1. ✅ **Generalized typed-link primitive (2026-06-16).** New `features/links/` feature.
   `links-proto`: `NodeRef {kind, id}` (verse/note/wiki/topic/entity/block/external,
   `kind:id` tokens) + `TypedLink {source, target, relation, confidence, visibility,
   provenance, note}`. Vocab: SKOS + argument-mapping + Breadcrumbs + scripture relations;
   ordinal `Confidence`; opt-in `Visibility` (Private default). `LinksService`:
   create/delete/get/`links_for(node)`/`graph(min_confidence, include_private)` — the last
   is the publishable, quality-filtered view. `links`: file-backed store
   (`<org>/links.jsonl`), mounted per org. 8 tests, clippy clean.
   - ✅ **Sub-node anchors (2026-06-17).** `NodeRef` gained `anchor` (`#[serde(default)]`,
     legacy tokens still round-trip) — generalizes Logseq/Obsidian `[[page#^block]]` to
     verses, words, and blocks uniformly. Token form `kind:id#anchor`; first `#` splits the
     anchor so verse-range ids (`John.3.16-18`) survive intact. Helpers: `block(uuid)`,
     `word(osis, n)` → `verse:John.3.16#word:5`, `with_anchor`, `has_anchor`. Block-kind
     refs resolve through the vault's `BlockIndex` (`lookup_str`/`block_preview`) at the
     service layer (needs the live `Vault`; kept out of wasm-clean proto).
   - ✅ **Resource / media anchors (2026-06-17).** `NodeKind::Song` (a song's
     YouTube/audio/lyrics are one node, addressed by anchor), `Relation::AlludesTo` (the
     echo relation worship lyrics use far more than `Quotes`), `song()`/`at(secs)` helpers,
     and an `Anchor` classifier (`Whole`/`Timestamp`/`Word`/`Block`/`Region`/`Span`) so the
     graph/UI renders seek-chips vs region-jumps vs line-spans. PDF region + media timestamp
     **geometry** lives in a per-resource annotation sidecar keyed by the anchor (Logseq's
     two-layer model), not on the wire. Full design + Recall/Logseq research:
     **`plans/resource-annotations.md`**. 9 tests, clippy clean.
2. ✅ **Node properties (2026-06-16).** `vault-live/property_schema.rs`: a reusable
   `node` base schema (`epistemic_properties`: `confidence` ordinal, `visibility`
   opt-in private-default, `maturity` seedling/budding/evergreen — `EnumWithMetadata`
   with colors + icons). Page kinds (person/area/task/daily, project via area) `extends`
   it, so every page inherits the three and the properties pane renders them.
3. ✅ **Bible link/tag data (2026-06-16).** OpenBible cross-references (`crossref.rs`,
   ~340k, signed votes) + topics (`topics.rs`, ~470k, bidirectional, vote-weighted),
   bundled into `<org>/resources/{crossref,topics}/` (CC BY). `ScriptureService`:
   `cross_refs(ref, min_votes)`, `topics_of(ref)`, `verses_for_topic(topic, limit)` —
   votes are the confidence signal. Verified: John 3:16 → Romans 5:8 (972); "money" →
   Hebrews 13:5.
   - ✅ **Entities as WIKI pages, not a parallel store (2026-06-16).** People/places are
     generated as `type: entity` wiki pages (reusing the wiki's entity infrastructure),
     bodies linking the verses (`[[Genesis 1:1]]`) so the wiki + link + backlink machinery
     handles verse↔entity. `entities::from_bible_data` parses BibleData CSVs (CC BY); the
     `generate_entity_pages` example emits them. 3090 pages → `wiki/Knowledge/Entities/`.
4. 🟡 **Quality-filtered graph view (core done 2026-06-16).** `view-knowledge-graph`:
   `GraphEdge` carries `relation` + `confidence`; `GraphFilterState` gains
   `min_confidence`/`typed_only`/`hidden_relations` + `edge_passes`/`apply_filters`;
   `build_link_graph(links, include_private)` turns `TypedLink`s into the
   verse↔verse↔entity graph (private dropped when publishing). Filters panel UI has the
   "Link quality" section (typed-only toggle + min-confidence selector). 26 tests.
   - ✅ **Live view (2026-06-17).** `/connections` route (`crates/ui/src/pages/connections.rs`)
     + `feeds::fetch_link_graph` (calls `LinksServiceClient.graph`) → `build_link_graph` →
     `KnowledgeGraphView` with the quality `GraphFilters` panel (confidence floor + typed-only).
     Nav tab "Connections" (Waypoints icon). Server already serves `LinksService`
     (apps/server lib.rs). Compiles native + wasm. Renders the 136 seeded song/sermon/verse
     links as the force-directed web.
   - ✅ **Focal-node subgraph (2026-06-20).** Clicking a node on `/connections` focuses the
     graph to it + its 1-hop neighbourhood (`focal_subgraph`), with a "Show all" chip; the
     quality filters still apply on top. (Verse→`/scripture` deep-link still pending — the
     reader has no verse route param.)
5. 🟡 **Publishing (export done 2026-06-18).** `links` `examples/publish_graph.rs`: exports
   the publishable subset (`graph(include_private=false)` → drops Private links) to
   `<org>/published/links.jsonl`, redacting private-`note:` endpoints (id → `private`) so a
   public edge can't leak a note path. Verified: 136 public links exported, 0 withheld (the
   seeded analysis is all Public; private watch-view journal notes would be withheld). The
   `/connections` page with the private toggle off is the viewer.
   - ✅ **Static HTML export (2026-06-18).** `publish_graph` also emits a self-contained
     `published/index.html` — the public interconnections grouped by source node (relation +
     target + confidence + note), inline CSS, no JS. 136 links / 65 groups.
   - ⬜ **Remaining:** wire into the federation/vault-publisher path; node-level visibility
     (publish-time lint for public→private targets beyond `note:`).
6. 🟡 **Authoring (2026-06-17/18).** Two paths landed: (a) the `/watch` view creates typed
   links live ("add note/clip at current time" + optional verse/topic target); (b) the
   `sync_vault_links` example turns `[[wikilinks]]` in note prose into `note→verse`/`note→video`
   links; and (c) **server-side on save (2026-06-20)** — `apps/server/src/link_sync.rs` spawns
   a task subscribed to the vault `VaultEvent` broadcast that re-syncs each saved note's
   `[[verse]]` wikilinks into `note→verse` links (verse case; replaces only `vault-link-sync`
   provenance, `Visibility::Private`). So the graph stays live without running the example.
   ⬜ Remaining: a general in-app "link this to that" picker (relation + confidence); extend the
   server sync to video-clip refs.

## Open design questions (need the user)

- Build order: **Bible data first** (1) then generalize, or **generalized primitives first**?
- Confirm **opt-in publishing** (private by default) — research strongly recommends it.
- Adopt the proposed **vocabularies** (maturity/confidence/visibility + relation set) as-is?
- Where typed links live: extend the **wiki `GraphEdge`** model, or a dedicated
  `links` feature trio that the vault/wiki/scripture all write into? (Leaning: a small
  shared `links` feature so verses, notes, and wiki entries all use one typed-link store.)

## Sources
OpenBible.info (topics + cross-refs, CC BY, weighted); STEPBible TIPNR (CC BY); Nave's
(BradyStephenson, CC BY). Gwern epistemic axes; Maggie Appleton digital gardens; Andy
Matuschak evergreen notes; Breadcrumbs/Juggl typed links; SKOS; argument mapping;
nanopublications / RDF-star; W3C Web Annotation; Quartz/Obsidian Publish/Logseq.
