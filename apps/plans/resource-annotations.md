# Resource Library + timestamped / region annotations

> Status: **design proposal** (2026-06-17). Generalizes the Bible-as-resource pattern to
> any primary source — songs, PDFs, books, videos, podcasts — that the vault/wiki can
> *annotate* and *link into* without editing. Grounded in research of Recall and Logseq
> (this session) and the typed-link primitive already built (`plans/knowledge-primitives.md`).
> First heavy user: scriptural analysis of a worship song (see worked example below).

## The goal (from the user)

- A song (or PDF, book, video) is a **resource** — like the Bible already is — that lives
  read-only in the Resources Library and is *linked into*, never edited.
- Make **timestamped annotations** on a YouTube video / audio file / song, **region
  annotations** on a PDF, and **span annotations** on plain lyrics/text.
- Do deep analysis — *"everywhere the lyrics reference a biblical concept, connect it with
  links throughout the entire Bible"* — so the annotation graph reveals the spirituality of
  the source.

## Research — how the field does it

### Recall (docs.recall.it) — the coarse end
- A **"card"** is any saved resource (article / YouTube / podcast / PDF / note) — one node.
- Graph edges: manual `[[` links + AI-extracted concept references + content references.
  Nodes colored by tag. A spaced-repetition / quiz layer sits on top.
- **Tellingly weak on anchoring:** *"Timestamps are not included"* for podcasts/TikTok. Recall
  links at **whole-resource** granularity, not moment-level. → We aim higher; their useful
  ideas are the *unified resource-card node*, *AI-extracted concept links*, and the
  *tag-colored graph + review layer*.

### Logseq — the rich end (the model to borrow)
The **two-layer** design is the key takeaway — a compact reference in the graph, the
geometry in a sidecar:
- **Asset** = a block tagged `:logseq.class/Asset`, props `:logseq.property.asset/{type,
  checksum, external-url}`; identity = block UUID; file at `assets/{uuid}.pdf`.
- **PDF highlight** geometry lives in a per-asset **`.edn` sidecar** (`assets/<key>.edn`):
  ```edn
  {:highlights [{:id #uuid "…" :page 1
                 :position {:bounding {:x1 131 :y1 336 :x2 483 :y2 399
                                       :width 574.0 :height 574.0}
                            :rects [ … ] :page 1}
                 :content {:text "Duke School" :image 1752179374600}  ; image = area PNG ts
                 :properties {:color "yellow"}}]
   :extra {:page 2}}
  ```
  Coordinates are **scaled PDF space** (0..page_w/h), not viewport px — survives zoom. Text
  highlight = `content.text`; area highlight = captured PNG at `assets/{key}/{page}_{id}.png`.
- **Annotation ↔ block:** each highlight auto-creates a block whose **UUID = the highlight
  id**, tagged `Pdf-annotation`, with the full highlight map in `:logseq.property.pdf/hl-value`
  and an `:logseq.property/asset` link back to the PDF. Block-ref form `[[uuid]]`. Opening it
  sets `:pdf/ref-highlight` → the viewer scrolls + flashes.
- **Media timestamp:** the `{{youtube-timestamp 125}}` macro — integer seconds; parser takes
  `HH:MM:SS` / `MM:SS` / raw; click → `player.seekTo(secs)`; embeds support `{:start secs}`.
  No media-fragment `#t=`. Stored inline in block content, no separate entity.

## Our model — map onto the typed-link primitive

The split falls straight out of Logseq:

1. **Graph layer — the anchor string (already built).** A `NodeRef` is
   `kind:id#anchor` (`plans/knowledge-primitives.md` §1). For resources:
   - `song:keep-on-finding-more#t:90` — a recording moment (seek).
   - `song:keep-on-finding-more#chorus.L1` — a lyric line/section span.
   - `resource:books/mere-christianity#p42.h3` — PDF page 42, highlight 3.
   - `verse:John.3.16#word:5` — sub-verse word (word study).
   `NodeRef::anchor_kind()` → `Anchor::{Whole, Timestamp(secs), Word(n), Block(id),
   Region{page,id}, Span(label)}` classifies the string for display + navigation. **Done in
   `links-proto` this session** (+ `NodeKind::Song`, `Relation::AlludesTo`, `song()`/`at()`
   helpers). Wasm-clean: no geometry on the wire.

2. **Detail layer — the annotation sidecar (to build).** Rich geometry keyed by the anchor
   id, one sidecar per resource (Logseq's `.edn`, as JSON for us):
   `<org>/resources/<…>/<slug>.annotations.json`:
   ```json
   { "anchors": {
       "p42.h3": { "page": 42, "bounding": {"x1":..,"y1":..,"x2":..,"y2":..,"w":..,"h":..},
                   "rects": [ … ], "text": "…", "color": "yellow", "image": "p42_h3.png" },
       "t:90":   { "secs": 90, "label": "Tag — '1/3' walk-up under 'more'" } } }
   ```
   The `links.jsonl` link carries the anchor *key*; the viewer resolves geometry here. Keeps
   the link store small and the proto portable.

3. **Resource manifest.** A resource is a read-only Library entry (frontmatter `type:
   resource`, `resource_kind: song|pdf|book|video`, `media: [{kind, provider, url}]`,
   `readonly: true`). Songs → `<org>/resources/songs/<slug>.md` (lyrics + Nashville chart with
   `^section.Ln` line anchors). PDFs/books → the file + its `.annotations.json` sidecar.

## Build order

1. ✅ **Anchor keystone + resource node kinds (2026-06-17).** `NodeRef.anchor`, `Anchor`
   classifier, `NodeKind::Song`, `Relation::AlludesTo`, `song()/at()/word()` helpers — all in
   `links-proto`, 9 tests, clippy clean. (See `plans/knowledge-primitives.md` §1.)
2. ✅ **Resource manifest + annotation sidecar types (2026-06-17).** New `features/resources/`
   (leaf crate, links-shaped: types + native store, no UI/RPC yet). `types` (wasm-clean:
   `Resource`/`MediaRef`/`ResourceKind`, `Annotation`/`AnnotationFile`, `Geometry`
   {Timestamp, PdfRegion+`Rect`} mirroring Logseq's highlight), `manifest` (parse `type:
   resource` frontmatter), `sidecar` (read/write `<slug>.annotations.json`), `build` (specs →
   `TypedLink`s + sidecar — the headless authoring path), `resolve` (`Anchor` →
   seek/region/span — read path, Logseq's `open-block-ref`). 7 tests, clippy clean.
   - ✅ **Seed materialized.** `examples/seed_songs.rs` wrote **two** worship songs into the
     live vault: *Keep On Finding More* (72 links) + *A Forgiving God* / Prodigal Son (41
     links) = **113 `song:slug#anchor → verse:osis` typed links** in `<org>/links.jsonl`
     (`AlludesTo`/`Quotes`/`Mentions` + confidence, `Public`, provenance-stamped) and **42
     annotations** across two sidecars. Idempotent by `source_ref`.
   - ✅ **Block anchors + walker (2026-06-17).** `resolve_with(file, node, block_preview)`
     resolves both a block *node* (`block:uuid`) and a block *anchor* (`note:p.md#^uuid`) to a
     preview via an injected closure — the caller wires it to the vault's new
     `BlockIndex::preview_str(&vault, uuid)` (added in `vault-live`), so `resources` stays
     vault-free. `walker::walk(root)` loads every `type: resource` manifest under
     `resources/**`. 
   - ✅ **Sermon analysis (2026-06-17).** `NodeKind::Sermon` + `NodeRef::sermon()`;
     `ResourceKind::Sermon`; a `transcript` module (`Transcript`/`TranscriptSegment`,
     `<slug>.transcript.json` sidecar, `at(secs)`/`text_between`); `Target` generalized to any
     `NodeRef` (`Target::topic`); `AnnotationSpec::moment(secs,…)`. Seeded *"God Restores
     Broken People"* (Crossroads, 1 Peter 5): 1256-cue transcript → sidecar, **23 timestamped
     links** (`sermon:slug#t:<secs>` → 12 verse citations `Quotes`/`AlludesTo` + 11
     key-moment `topic` tags). Study note in the vault. Total graph now **136 links** across 2
     songs + 1 sermon.
   - ⬜ **Remaining:** PDF/region geometry capture (needs the viewer, slice 3).
3. 🟡 **Graph view done; media player pending (2026-06-17).**
   - ✅ **`/connections` graph** — the live typed-link web (`LinksService.graph` →
     `build_link_graph` → `KnowledgeGraphView` + quality `GraphFilters`). See
     `plans/knowledge-primitives.md` §4. Renders all 136 song/sermon/verse/topic links.
   - ⬜ **Media player + lyric/timestamp UI.** A resource view: YouTube/audio embed with a
     timestamp gutter (`Anchor::Timestamp` → `seekTo`), lyric chart with per-line annotation
     chips (`Anchor::Span`), PDF region overlay (`Anchor::Region`, scaled→viewport like
     Logseq). "Annotate at current time" → creates a `TypedLink` from `song:…#t:<now>`.
4. ⬜ **Authoring flow.** Select a lyric span / scrub to a moment / drag a PDF box → pick
   relation (`alludes-to`/`quotes`/`mentions`) + confidence + target (verse/topic/wiki) →
   write the link (+ sidecar geometry). The private-journal + publish path from
   `plans/knowledge-primitives.md` §5–6 applies unchanged.

## Obsidian-style vault integration (2026-06-18)

The features surface **inside the `/vault` editor workspace**, not just as separate top-nav
tabs (the user's direction: "integrate into the Editor vault UI, just like Obsidian"). The
vault content pane branches on the open file:
- a **`.base`** file → renders its live tables in place of the editor (`pages::bases::BaseDoc`,
  a reusable per-base component; row-click opens the target note in the same vault view).
- a **`type: video`** note → opens the watch **player** (`pages::watch::WatchView`; the note's
  basename is the YouTube id) — embed + timestamped notes + transcript, all in the vault.
- else → the editor (unchanged).
Detection uses the tree index's `page_type` + the path extension. native + wasm + clippy clean.
⬜ Remaining integrations: a connections-graph panel in the vault; verse `[[…]]` →
scripture reader inline; optionally retire the now-redundant `/bases`,`/watch` top tabs
(kept for now as add-new entry points). Inline `.base` is view-only (no raw-source edit yet).

## Native Bases for filtering (2026-06-17)

The user filters this data through **native Obsidian Bases** (`.base` files), one per
category — Songs, Sermons, … The main vault is rooted at `<org>/vault/` **only**
(`apps/server` line 545: `org_root.vault_dir()`); `resources/` and `wiki/` are *not*
indexed by it. So Bases query the **study notes** in `vault/Scripture/` (the vault-side
record), not the resource manifests. Each study note carries `kind: song|sermon` (+ `tags`)
as the discriminator; bases filter on it.

**Canonical Obsidian syntax — these files open in real Obsidian AND Task's engine.** Authored
+ verified (`vault-obsidian` `examples/verify_bases.rs`): `Songs.base` (`kind == "song"` +
`file.inFolder("Scripture")`; views All songs / By artist / Worship set via
`file.hasTag("worship")`), `Sermons.base` (`kind == "sermon"`; By passage), `Scripture
Studies.base` (`type == "study"`; By kind). All execute (2 songs / 1 sermon / 3 studies).
- **Parser compat fix (2026-06-17).** Real Obsidian (help.obsidian.md/bases/syntax) uses
  `groupBy: { property, direction }` (object) and `file.inFolder()` — this repo's parser only
  read a bare-string `groupBy` and lacked `inFolder`. Patched `vault-live/src/bases.rs`:
  `groupBy` now accepts the object form (and still the string); added `file.inFolder(path)`.
  16 bases tests pass.
- **Schema notes** (the repo's parser, = real Obsidian, ≠ the repo's *own* example `.base`
  files which use ignored keys): leaf filters are **string expressions** (`'kind == "song"'`,
  `file.hasTag("x")`), NOT `{property, equals}` maps; columns are **`order:`** NOT `columns:`;
  view-level filter is **`filters:`** (plural); direction `ASC`/`DESC`; tags via
  `file.hasTag()`. The `kind: song|sermon` + `tags` discriminators live on the study notes.
- ✅ **In-app renderer (2026-06-17).** `VaultSync::base_views(vault_id, base_path) ->
  Vec<BaseView>` added to the RPC (vault-proto DTOs `BaseView{name, view_type, columns,
  groups}` → `BaseGroup{label, rows}` → `BaseRowView{path, basename, title, cells}`).
  Implemented in `vault-live/sync.rs`: open Vault → `frontmatter_json` per page (serde_yaml on
  the `---` block) → `BaseRow::from_parts_full` → `bases::execute_view`, projecting each
  `order` column via the new pub `bases::cell_value` (arrays comma-joined) + `ViewKind::as_str`.
  UI: `feeds::{fetch_bases, fetch_base_views}` + `/bases` page (`pages/bases.rs`: base-picker
  chips + nav tab "Bases", Table2 icon). Verified end-to-end against the live vault
  (`vault-live examples/run_base_views.rs`). Compiles native + server + wasm; clippy clean.
  **Needs a server rebuild** (proto schema skew).
  - ✅ **Obsidian core view types (2026-06-17).** Obsidian's core built-in views are **table**
    (v1.9), **cards** (v1.9, grid + cover image), **list** (v1.10), **map** (v1.10, needs the
    Maps plugin) — *not* board/calendar. Added `ViewKind::Cards` (parses `type: cards`;
    `gallery` kept as an alias) to `vault-live/bases.rs`. `pages/bases.rs` dispatches on
    `view_type`: `BaseTableBody` / `BaseCardsBody` (card grid: title + labeled fields) /
    `BaseListBody` (bulleted, title + `·`-joined secondary); board/calendar/unknown fall back
    to a table; `map` would need coords + a tiles dep (deferred). All respect `groupBy` labels
    and row-click → `/vault`. Confirmed: `Songs.base` now has table/cards/list views, all
    emitted by the engine.

## Video clips + references (2026-06-17)

Generic YouTube-video notes with **clip** (timestamp-range) references. The chosen design:
- **`NodeKind::Video`** (generic; Song/Sermon are specializations). `NodeRef::video(slug)`;
  the video note holds the URL.
- **Clip anchor = a timestamp range.** `Anchor::Clip { start, end }` (seconds). Token
  `video:my-talk#t:263-983`. Helpers `NodeRef::clip(263, 983)` /
  `clip_from_timecode("4:23","16:23")`; point stays `at(secs)` → `#t:90`.
- **Timecodes.** `parse_timecode` accepts `263` / `4:23` / `1:04:23`; `format_timecode` is the
  inverse. `Anchor::parse` reads either seconds or `mm:ss` in the `t:` anchor, so
  `#t:4:23-16:23` classifies too. (links-proto, 10 tests, clippy clean.)
- **Reference syntax** (the user's `[[VideoLink:4:23-16:23]]` ask): use `#`, not `:`, as the
  separator — `:` already splits `kind:id` and appears inside `mm:ss`. So the human wikilink
  is **`[[my-talk#4:23-16:23]]`** (Obsidian-style, like `[[Note#^block]]`); a point is
  `[[my-talk#1:30]]`. Canonical machine token: `video:my-talk#t:263-983`.

### ✅ Watch view (2026-06-17)
`/watch?v=<id>&node=<token>` (`crates/ui/src/pages/watch.rs`) — embeds the YouTube IFrame
player (`enablejsapi=1`) and drives it over the postMessage channel (the proven
`wiki_source.rs` pattern): seek-on-click of every moment, and **"Add note at current time"**
reads `getCurrentTime` back via a `message`/`infoDelivery` listener (`eval.recv`) → writes a
`TypedLink` from `<node>#t:<secs>` (note in `link.note`). Moments come from
`LinksService.links_for(node)` (new `feeds::{fetch_links_for, create_link}`), so it works for
any `video:`/`sermon:`/`song:` node — **the sermon is a ready example**
(`/watch?v=YMypVgZXFIU&node=sermon:god-restores-broken-people` shows its 23 timestamped
notes). No-video → a paste-a-URL landing (`youtube_id` extractor). Nav tab "Watch" (Youtube
icon). Compiles native + wasm; clippy clean. **Needs a server rebuild** (links-proto changed).

### ✅ Watch authoring (2026-06-18)
- **Save to library** → `feeds::save_video_note` writes a `type: video` vault note
  (`Videos/<id>.md`, `kind: video` + `url` + `tags`), so the video shows in `Videos.base` and
  `[[id]]` resolves. (Title defaults to the id; rename in the note.)
- **Clip authoring**: "Mark in" captures the playhead → `clip_in`; the Add button then writes
  `node#t:in-out` (ordered) — a clip — else a point `node#t:secs`.
- **Optional target**: a "link to" field → `parse_target` (full `kind:id` token / OSIS verse
  `John.3.16` / topic slug) sets the link target (relation `alludes-to`), else the video
  itself (`mentions`).
- `Videos.base` authored. native + wasm + clippy clean.

### ✅ Transcript in the watch view (2026-06-18)
New **`resources-proto`** trio: `#[architect::rpc] ResourcesService { transcript(rel_path) ->
TranscriptDoc }` (wasm-clean DTOs `TranscriptDoc`/`TranscriptSegment`, Facet + Reborrow);
`resources::ResourcesBackend` reads `<org>/resources/<rel_path>` (path-traversal guarded),
mounted in `apps/server` (schema-stamp + `.with(...)`). UI: `feeds::fetch_transcript` +
`/watch` transcript panel (`transcript_rel_path` derives `sermons/<slug>.transcript.json`
from the node) — a scrollable cue list, **click a line → seek there + pre-fill the note**.
Verified the sermon sidecar (1256 cues) matches the DTO. native + server + wasm + clippy
clean. **Server rebuild needed** (new service → schema skew).

### Editor resolution — finding (2026-06-18)
Inline `[[slug#4:23-16:23]]` → play-chip rendering is **NOT feasible in `crates/ui`**: the
wikilink live-preview tokenizer + decoration rendering live entirely in the external **Editor
repo** (`editor` git dep); `crates/ui` only supplies the `VaultLookup` resolver *data*, not
the rendering. A true inline chip needs an Editor-repo change (a custom-wikilink-renderer
hook). The wikilink parser *does* already split `[[target#anchor]]` (anchor = `4:23-16:23`),
so the data is there.

### ✅ Vault link-sync — the in-scope alternative (2026-06-18)
`links` `examples/sync_vault_links.rs` parses every vault note's `[[wikilinks]]` (via
`vault_obsidian` `LinkRef`) into typed links: `[[John 3:16]]` → `note → verse:<osis>`
(`mentions`, via `scripture_proto::VerseRange::parse`), `[[<video>#4:23-16:23]]` →
`note → video:<slug>#t:…` (`cites`). So `[[…]]` references in prose become real edges in
`/connections` + the watch view — without touching the editor. Private (notes are the private
layer). Idempotent (`source_ref: vault-link-sync`). **124 note→verse links** synced from the
study notes (260 links total now). ✅ Now also runs **server-side on save**
(`apps/server/src/link_sync.rs`, verse case) — see knowledge-primitives.md §6.

### Remaining (authoring polish)
- **oembed title** on save (needs a server fetch or CORS-friendly path).
- Transcript ingest for *new* videos (yt-dlp/whisper server-side) so non-sermon videos get cues.
- Inline editor play-chips — deferred to an Editor-repo renderer hook.

## Worked example (seed data, done this session)

- Resource: `<org>/resources/songs/keep-on-finding-more.md` (read-only chart, `^section.Ln`).
- Study note: `<org>/vault/Scripture/Keep On Finding More — Scriptural Analysis.md` — every
  lyric line annotated against Scripture with relation + confidence; the thesis is
  *seek→find* ([[Jeremiah 29:13]], [[Matthew 7:7-8]]); the keystone line "from Eden to Zion"
  brackets [[Genesis 2:8-10]] → [[Revelation 22:1-2]] (the Eden river restored). These become
  `song:keep-on-finding-more#<span>` → `verse:<osis>` typed links once §2 lands.

## Sources
Recall docs (docs.recall.it — resource cards, AI concept links, no moment anchors). Logseq
source (`~/Development/research/logseq`): `extensions/pdf/{assets,core}.cljs` (highlight shape
+ `.edn` sidecar + scaled coords), `handler/assets.cljs` (asset model),
`extensions/video/youtube.cljs` (`{{youtube-timestamp}}` + `seekTo`). W3C Web Annotation
(selector/target model) as the formal backstop.
