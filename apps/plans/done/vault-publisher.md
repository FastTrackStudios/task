# Vault Publisher (Quartz-equivalent for `.loro` vaults)

A Rust binary that takes a Task-architect `.loro` vault (the file
emitted by the **Export** chip) and produces a static documentation
site — the way Quartz takes a folder of Markdown and produces
[quartz.jzhao.xyz](https://quartz.jzhao.xyz).

## Why

- Publish version-controlled docs from the same vault you edit.
  Push the `.loro` snapshot to git, CI regenerates the site.
- Read-only audience doesn't need to install the app, doesn't
  need wasm download, doesn't need a sync server.
- Static HTML deploys anywhere (GitHub Pages, Netlify, S3+CDN,
  any nginx).
- Same source-of-truth as the live app — no duplicate "docs"
  copy that drifts.

## Reference: Quartz architecture

Cloned to `~/Development/research/quartz/`. Stack:

- **Input**: Markdown files in `content/` (Obsidian-flavored).
- **Pipeline** (TS, runs in Node):
  1. `processors/parse.ts` — Markdown → MDX/HAST AST
  2. `processors/filter.ts` — drop drafts, private, etc.
  3. `processors/emit.ts` — render AST + Preact components → static HTML files
- **Plugins**:
  - `transformers/` — modify content during parse (frontmatter, OFM links, GFM, LaTeX, syntax highlighting)
  - `emitters/` — write specific files (contentPage, tagPage, folderPage, contentIndex.json for search, ogImage, favicon, sitemap)
  - `filters/` — boolean predicates for which content to publish
- **Components** (Preact JSX): Backlinks, Graph, Search,
  Explorer, ArticleTitle, ContentMeta, TagList, TableOfContents,
  Breadcrumbs, Header, Footer, Darkmode, RecentNotes
- **Output**: Static HTML site, SPA-flavored navigation by
  default. Search index is precomputed JSON, fuzzy-matched
  client-side.
- **Build**: `npx quartz build` → walks content, runs
  transformers, runs emitters, writes to `public/`.

Features that make Quartz nice to copy:
- Auto backlinks page per note
- Auto graph view (force-directed, JSON-driven)
- Search (precomputed index, client-side fuzzy)
- Smart explorer (collapsible folder tree)
- Wikilink resolution (`[[Page]]` → linked HTML)
- Tag pages aggregating all notes under each tag
- Last-modified dates from git
- SPA-style page transitions
- Dark mode toggle, mobile responsive
- Auto OG images (social previews)

## Our equivalent — `task-publish`

### Stack

- **Rust binary** at `apps/publish/` (CLI + library)
- **Dioxus + dioxus-ssr** for rendering. Components live in
  `components.rs`, designed to be context-free so they SSR
  cleanly. Future: lift to a shared `publish-components` crate
  used by both the live app (with interactivity) and the
  publisher (read-only).
- **Future hydration path**: ship the same components plus a
  client-side wasm bundle that hydrates static HTML into a live
  Dioxus app — search, graph, dark-mode toggle become
  interactive without rewriting components.

### Use beyond Task-architect

This publisher is built so it can drive **arbitrary Dioxus-based
doc sites**, not just task vaults. The Block/Page types
currently come from `knowledge_proto` but the components
themselves only consume their data shape — a future generalization
to a `PublishablePage` trait would let external content sources
(raw markdown trees, RSS, even arbitrary Dioxus components a
user writes) drive the same publisher pipeline. Marked as a
v2 generalization; v1 is vault-specific.
- **Markdown parsing**: reuse `knowledge_ui::inline_md::parse_inline`
  for inline elements; reuse `knowledge_ui::outliner::{parse_callout,
  parse_table, parse_footnote_def}` for block-level features.
- **Block listing**: `knowledge_crdt::{BlockRepoLoro, PageRepoLoro}`
  via crudcrate `list()`.
- **Template-free**: HTML rendered as Rust format strings, since
  the page shape is fixed (sidebar + article + meta).

### Architecture

```
.loro snapshot file
    │
    ▼
CrdtDoc::ephemeral().loro().import(snap_bytes)
    │
    ▼
PageRepo::list() → Vec<Page>
BlockRepo::list() → Vec<Block> (filter per page)
    │
    ▼
For each Page:
    blocks = blocks_for(page.id)
    html = render_page(&page, &blocks, &nav_index, &backlinks_index)
    write public/<slug>.html
    │
    ▼
Build index.html (page list)
Build search.json (block snippets + page slugs)
Build backlinks.json (target-uuid → source-uuid[])
Copy assets/style.css
```

### Output structure

```
public/
├── index.html              ← landing page, lists all top-level pages
├── <slug>/index.html       ← per-page (clean URLs, no .html ext)
├── tags/<tag>/index.html   ← tag aggregation pages
├── search.json             ← precomputed search index
├── graph.json              ← node + edge data for the graph view
└── assets/
    ├── style.css           ← single bundled stylesheet
    └── client.js           ← optional: search box + dark-mode toggle
```

### Phases

**Phase 1 — Hello, static site (~½ day)**
- New crate `apps/publish` with binary `task-publish`
- CLI: `task-publish input.loro --out public/`
- Reads snapshot, lists pages, renders each as a single
  `<article>` of inlined block content
- Index page lists every page as a link
- One CSS file, basic light/dark theme, monospace body font
- **Deliverable**: from a vault file, get a browsable static site

**Phase 2 — Wikilinks + backlinks** ✅ shipped
- Wikilinks resolved against `basename → slug` index. Render
  `<a class="wikilink">` for resolved, `<span class="wikilink broken">`
  for unresolved (no href, dotted underline).
- `BacklinkIndex` precomputed once (target_slug → BacklinkEntry[]).
  `BacklinksPanel` Dioxus component renders below each article
  with a "Linked by" header.
- Tag aggregation: `tags::extract_tags` walks each block content
  for `#tag` patterns (left-bounded by whitespace/start, char
  class `[A-Za-z0-9/_-]`). `TagIndex` maps tag → pages, emits
  `/tags/<tag>/index.html` per tag plus `/tags/index.html`
  listing all tags with counts.
- New sidebar links: Home / Graph / Tags
- 3 tag tests + 1 backlinks-via-graph test added.

**Phase 3 — Search** ✅ shipped
- `search::build` precomputes `[{slug, title, snippet}]` per
  page. Snippet = first ~240 chars of joined block content,
  whitespace-collapsed.
- Emitted as `assets/search.json` at build time.
- `assets/search.js` (~50 lines, no deps): case-insensitive
  substring filter, hits limited to 20, results render under
  the search box with title + snippet preview. `/` keyboard
  shortcut focuses the input.
- Search box lives in the sidebar via the `Sidebar`
  component — present on every page.

**Phase 4 — Graph (~1 day)** ✅ shipped
- `graph::compute(pages, blocks)` walks every block via the
  inline parser, finds wikilinks, dedupes per-page, builds
  `{nodes: [{id, label, degree}], edges: [{source, target}]}`.
- Emits `assets/graph.json` and `assets/graph.js` (~120-line
  vanilla force simulation + canvas renderer; no D3, no Pixi).
- `GraphView` Dioxus component renders an empty `<canvas>` +
  defer-loads the simulation script.
- New `/graph/` page in the site, linked from every page's
  sidebar.
- 4 unit tests cover edge dedup, self-link skip, broken-link
  skip, basic node degree.
- Inspired by Quartz's `Graph.tsx` (D3-force + Pixi) and
  Logseq's graph view, trimmed to a no-dep implementation.
  Good up to ~500 nodes via O(n²) pairwise repulsion; bigger
  vaults can swap in a Barnes-Hut quadtree or upgrade to D3.

**Phase 5 — Polish**

✅ **Code-block syntax highlighting** — `syntect` server-side
with the bundled Sublime grammars. Per-token `<span style=…>`
output via `dangerous_inner_html`. Theme: base16-ocean.dark
(token colors picked at build time; CSS shell adds the
surrounding card frame). Covers the long tail of languages
out of the box.

✅ **Sitemap.xml + RSS feed** — emitted when `--base-url` is
passed. Sitemap covers `/` + every page slug with `lastmod`.
RSS 2.0 feed with the 50 most-recently-modified pages, each
with a 240-char snippet from the joined block content.

✅ **Math via KaTeX** — client-side auto-render via the CDN
build (~300KB, lazy-loaded only on pages that look like they
contain math). Falls back gracefully when offline + uncached
— text renders as raw `$…$`. Self-hosters can swap the CDN
URLs in `katex_loader.js` for local copies.

✅ **`PublishablePage` trait scaffold** — defined in
`apps/publish/src/source.rs` so external content sources
(plain Markdown trees, Notion exports, custom Dioxus
components) can drive the same publisher pipeline. Defined
but not yet wired through `site::build` — that refactor
follows when a second input source lands.

✅ **Mobile sidebar drawer** — hamburger button in header,
off-canvas slide-in sidebar on mobile (`max-width: 760px`),
dimmed backdrop intercepts outside clicks to close.
Implemented via the CSS "checkbox hack" (no JS); animated
hamburger → "x" transition.

Pending:
- OG images per page (compose title cards with `image` crate)
- Dark mode toggle (currently auto via `prefers-color-scheme`;
  manual toggle would store override in localStorage)

### CI integration (later)

```yaml
# .github/workflows/publish.yml
on:
  push:
    paths: ['vault.loro']
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo run -p task-publish -- vault.loro --out public
      - uses: peaceiris/actions-gh-pages@v3
        with:
          publish_dir: public
```

### Open questions

- **"Dodeca"** — user mentioned this alongside Dioxus as a
  possible framework. Don't recognize the name. Need to ask
  whether this is a typo for "Dioxus", or a different
  framework I should look up before scaffolding. Defaulting to
  pure-Dioxus (or no framework at all for v1) until clarified.
- **HTML content fidelity** — Phase 1 ships block-level rendering
  via direct String building. Do we need feature parity with the
  live app's `BlockView` (callouts, tables, footnotes, GFM)? If
  yes, we either duplicate that logic in publisher (acceptable —
  the rendering is small) or refactor BlockView to be SSR-able
  (bigger).
- **Per-page assets** — embedded images in blocks. v1 assumes
  inline `![](url)` resolves to external URLs. Local assets
  (drag-and-dropped images, file attachments) need to be
  exported alongside the .loro and resolved to relative paths.
  Out of scope for v1.
