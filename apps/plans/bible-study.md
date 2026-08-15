# Bible Study — Feature Plan

> Status: **research / design draft** (2026-06-16). Branch `feat/biblical-research`,
> worktree `Task-wt-biblical`. Multiple agents will pick up slices from here.
>
> Goal: study the Bible the way you'd want to in Obsidian, but without Obsidian's
> rough edges — every verse (and every *word*) directly linkable, translations
> comparable, original languages and lexicons inline, ESV-study-Bible-grade
> annotations — all in **raw markdown an LLM can read**, wired into the existing
> **vault** (personal thoughts/experience) and **LLM wiki** (facts/deep studies).

---

## 0. The thesis (what we're actually building)

**Three tiers, one-directional linking** — and a Bible is just the first citizen of
the third tier:

| Tier | Holds | Backed by |
|---|---|---|
| **Vault** | your personal thoughts, experiences, questions, devotional notes | `features/vault/*` (markdown + block IDs, `<org>/vault/`) |
| **Wiki** | curated LLM knowledge — facts, entity pages, deep studies | `features/wiki/*` (`<org>/wiki/Knowledge/`) |
| **Resources Library** *(new tier)* | **primary sources**: books, epub, txt, markdown — and the Bible | `<org>/resources/` — large read-only corpora, link-in only |
| **Annotations** *(cross-cutting)* | verse/locator-anchored study notes, highlights, typed like NET/ESV | hybrid: structured anchors in CRDT, prose in vault markdown |

```
vault/      your notes        ──┐ link into
wiki/       curated knowledge ──┤ link into
resources/  primary sources   ◄─┘  self-contained; the link-in target
```

### The Resources Library (the key architecture)

Large primary-source material does **not** belong in the vault or the repo — it's a
separate **Resources Library** tier (promoting what the wiki called `raw/sources/` to a
first-class layer). It holds books in markdown / epub / txt and the Bible. Both the
vault and the wiki link *into* resources; resources never link out (same one-directional
rule as `wiki/Knowledge/`). We process **highlights/annotations** against a resource and
link to an **exact location** inside it.

The Bible is therefore not special — it's the **first resource type**. Its handler is the
`scripture` feature: a Bible resource is a folder of per-book USFM, its location key is a
[`VerseId`]; an epub resource would have its own locator (CFI / chapter+offset), a
markdown book its block IDs. Generic resource concerns (epub parsing, highlight
extraction, a uniform locator trait, a `resources` tier feature) are their own follow-up
slices — see §10.

> **Read-only + not-in-repo (confirmed).** Bible text is immutable from the UI; only
> system ingest writes it. The corpus is **installed into the resource library on disk**
> (`<org>/resources/bible/<TX>/`), which syncs to the server like the vault/wiki — it is
> never committed to the git repo (a full tagged translation is ~18 MB). Notes anchor to
> [`VerseId`], not to text, so they stay valid and shareable across translations.

Because this app already has a `BlockIndex` (`features/vault/vault-live/src/blocks.rs`,
`uuid → (page, offset)`, O(1)), **per-verse clean backlinks come for free** — which
is exactly the pain point that forces Obsidian users into 31k-file vaults. We get
granular backlinks *at chapter-file scale*. That single property dissolves the
whole "per-verse vs per-chapter" war the community has never resolved.

---

## 1. Use cases to satisfy (from the brief)

1. Read the Bible *in this app*, switching translations freely.
2. Link directly to **any single verse** from a note (`[[John 3:16]]`).
3. Link to a **specific word or phrase within a verse** (language study).
4. **Compare translations** side-by-side.
5. See the **original language** (Hebrew/Greek) with lemma + morphology + Strong's.
6. **Annotate** verses/ranges like an ESV Study Bible, in raw markdown.
7. Bible text is **entity-tagged**: a city/person in the text links to its wiki page.
8. Build **timelines** and **question/topic notes** that aggregate every relevant passage.
9. Wiki entries (locations, people, events) act as **sources/context** for reading.

---

## 2. Data model — the keystone

Everything hangs on a stable verse/word addressing scheme. Get this right first.

### 2.1 Verse addressing
- **Human / interchange key:** OSIS `osisID` string — `Gen.1.1`, ranges `Gen.1.10-12`.
  (CrossWire standard; STEP data already uses this style.)
- **Sortable primary key:** `BBCCCVVV` integer — `Gen 1:1 → 1001001`, `John 3:16 → 43003016`.
  Book (1–2 digits) + chapter (3) + verse (3). Used for ordering/range queries.
- **Block ID:** each verse is an addressable block (Logseq model) carrying its stable
  ID. Referencing a verse renders it **by transclusion** (live), never by copy — edit
  the source, every reference updates.

### 2.2 Sub-verse (word) addressing — the differentiator
- Each original-language word has a stable word ID (OSHB/MACULA already provide these).
- A note can anchor to `(verse, wordSpan)` — e.g. "the Greek behind *love* in John 3:16".
- This is **unsolved in Obsidian** (no native sub-verse anchor). It's a clear win and
  the foundation of language study.

### 2.3 Versification mapping
- Translations number verses differently (Psalm titles, the Ps 9/10 split, Jeremiah in
  LXX, Daniel additions, 3 John). A reference must resolve across editions.
- **Ship the Copenhagen Alliance `versification-specification` JSON mappings**
  (base = `org`, plus `eng/lxx/vul/rsc/rso`). Structure gives `maxVerses`,
  `mappedVerses` (`"PSA 9:22-39":"PSA 10:1-18"`), `excludedVerses`, `partialVerses`.
  Alternative: STEP **TVTMS** TSV.

### 2.4 Ingest format
- Bundle texts as **USFM** (the most widely distributed format — eBible.org), convert
  to **USJ** (USFM-as-JSON, USFM 3.1) via `usfm-grammar`. USJ → Rust structs / block
  store / markdown emission with no XML parser.
- Keep OSIS/USX/OSIS-XML as archival/interchange only, not the internal model.

---

## 3. Data sources (all bundleable unless noted)

### Bundle offline — public domain / CC0 / CC BY (no API key needed)
- **Reading texts:** **WEB** (World English Bible, PD) and **BSB** (Berean Standard
  Bible, **CC0** since 2023) are the zero-restriction modern-English defaults. Add
  **KJV** (PD except UK Crown copyright — prefer WEB/BSB for UK/EU) and **ASV/YLT** as
  classics. Source: **eBible.org USFM** → `usfm-grammar`.
- **Original language:** **STEP TAGNT/TAHOT** (amalgamated Greek/Hebrew, disambiguated
  Strong's + morphology, **CC BY 4.0, flat TSV** — the workhorse) + **OSHB/morphhb**
  (Hebrew word IDs) + **MACULA** (Clear Bible: syntax trees, senses, glosses — best for
  deep study). All bundleable with attribution.
- **Lexicons (all PD):** Strong's (openscriptures/strongs), BDB (Hebrew),
  Thayer's (Greek), STEP **TBESH/TBESG** brief + **TFLSJ** (full Liddell-Scott-Jones).
- **Interlinear/alignment:** **Berean interlinear** (PD, 2023) + OSHB word IDs.
- **Entities seed:** STEP **TIPNR** — every proper noun → forms + **genealogy +
  geolocation** (directly usable as Factbook-style entity seeds).
- **Cross-references:** **Treasury of Scripture Knowledge** (TSK, public domain).
- **Versification:** Copenhagen Alliance mappings (above).
- **NET Bible:** text + ~60k **typed footnotes** via labs.bible.org API
  (`?passage=...&formatting=full&type=json`, free non-commercial) — a ready-made
  typed-annotation corpus to model our apparatus against, and a generously-licensed
  translation.

> ⚠️ **Snapshot the SBLGNT license at ship time** — it moved from a restrictive EULA
> to CC BY 4.0; verify before bundling.

### Fetch via API only (copyrighted — never bundle)
- **NIV** *(priority — user wants this)*. **Tightly controlled by Biblica / Zondervan /
  HarperCollins** — among the hardest licenses in the space. Not reliably available on a
  free/standard API tier the way ESV is. Options to verify, in order of preference:
  1. **API.Bible** (American Bible Society) — confirm NIV is in catalog + cost/commercial terms.
  2. **Faithlife/Biblia API** — has NIV, but anti-compete + no-DB-extraction clause (read ToS).
  3. Direct **Biblica license** for heavier/commercial use.
  Whichever we use: NIV is **fetched per-passage, cached within license limits, never
  bundled or persisted as a redistributable file**. Notes anchor to verse IDs, so the
  vault stays shareable even though the NIV text isn't.
- **ESV** → ESV API (`api.esv.org`, free non-commercial, cache ≤500 verses / ½ book).
- **NLT / NASB / CSB / NRSV** + broad catalog → **API.Bible** (per-translation paid for
  commercial). Convenience PD live lookup without keys: **Bolls.life** or **GetBible v2**.

> **Copyright-clean by design:** annotations/notes anchor to **verse IDs**, not to
> copyrighted text. Licensed translations are fetched/optional. The vault stays legally
> shareable. (This is why Obsidian's "My Bible" plugin fetches instead of persisting.)

---

## 4. Feature set (prioritized)

Tags: **[must]** load-bearing first cut · **[high]** · **[nice]**.

### Foundation
- **[must]** Verse addressing scheme + permalinks (§2.1) — the keystone.
- **[must]** Strong's-tagged bundled text (STEP TTESV/TAGNT/TAHOT): every word →
  `H####`/`G####` + lemma + morphology.
- **[must]** Lexicon entries as KB pages — one page per Strong's ID (TBESH/TBESG/TFLSJ).
- **[high]** Versification mapping (§2.3).

### Reading & comparison
- **[must]** In-app reader, chapter-at-a-time, verse blocks — **read-only** (no user edit).
- **[must]** Translation switch + **side-by-side parallel comparison** (stable IDs, no
  link breakage on swap). Bundled PD texts (WEB/BSB/KJV) plus API-fetched **NIV/ESV**.
- **[high]** Translation-as-a-layer: one set of verse IDs, swappable text (bundled PD or
  API-fetched licensed).

### Word-level study
- **[must]** Click word → Strong's ID → lexicon entry (BLB/BibleHub/STEP chain).
- **[must]** **Every-occurrence concordance** (click lemma → every verse using it) — this
  is just a backlinks query over the tagged text (Englishman's Concordance model).
- **[high]** Reverse interlinear / hover-parse (English↔original highlight, lemma+morph).
- **[nice]** Translation-count breakdown (how a lemma is rendered, with counts).
- **[nice]** Sense-based lookup (Logos Bible Sense Lexicon) — no open dataset; LLM-tag later.

### Cross-references & annotations (the heart)
- **[must]** **TSK cross-reference graph**, phrase-keyed, with target verse text inlined.
- **[must]** **Typed annotations** (NET model): each note carries a category enum —
  `tc` text-critical · `tn` translator's · `sn` study · `map`. Filterable / color-codable
  / toggleable. Highest-leverage idea here.
- **[must]** **Two-level anchoring** (ESV model): section/passage notes (titled, span a
  range) + verse/word notes that quote the keyed text. Anchor =
  `(book, ch, vStart, vEnd, optional wordSpan)`.
- **[high]** Multi-anchor / reference-anchored notes (Logos): one note → several
  non-contiguous passages; appears at that ref across all translations.
- **[high]** ESV cross-reference notation grammar as typed refs (`ver.`, `ch.`,
  `[...]` thematic, `See`, `For…see` parallels).
- **[high]** Highlighting + saved-filter overlays (Logos Visual Filters).

### Entity / knowledge-base layer (your stated priority)
- **[high]** **Factbook-style entity pages** for people/places/things/events as
  first-class wiki notes; entities in the verse text become links. Seed from STEP
  **TIPNR**. This is the bridge into the existing wiki feature.
- **[high]** Entity cross-linking: one entity backs several views (a node is
  simultaneously a Factbook page, a Timeline node, an Atlas location).
- **[nice]** Atlas/geography (TIPNR has coordinates) — map view, hover → entity snippet.
- **[nice]** Timeline/chronology of events linked to verses + entity pages.

### Synthesis workflows
- **[high]** **Topic / question notes** that aggregate passages — a topic is a vault/wiki
  note collecting passages + sub-topics + related topics. This is exactly what a PKB does
  well (a topic = a note with backlinks/queries). Directly serves "I have a question and
  link to everything that helps answer it."
- **[high]** **Passage Guide panel** (Olive Tree "context-follows-cursor"): side panel
  resolving all cross-refs/notes/commentaries/entities for the focused verse — a fan-out
  query from the current verse anchor.
- **[nice]** Book/section intros + outlines (ESV template), inline charts/maps.

### Original-language power tools (advanced, later)
- **[nice]** Morphology search (we have STEP morph tags).
- **[nice]** Syntax/clause search (Accordance-style) — needs syntax-tree DB (MACULA);
  likely overkill for personal study.

---

## 5. Pain points we explicitly beat (vs Obsidian/Logseq)

1. **Per-verse backlink granularity without 31k files** — our `BlockIndex` gives each
   verse its own clean backlink set while chapters stay the file unit. *The* killer fix.
2. **Sub-verse word/phrase anchoring** — nobody does this well; §2.2.
3. **Graph that doesn't drown in scripture** — keep the scripture spine separate from the
   thought graph; filter scripture out of the graph by default.
4. **Native translation comparison** — parallel view, stable IDs, no link breakage.
5. **Original-language layer** — Strong's/Hebrew/Greek/lexicon inline; the biggest gap
   vs Logos-class tools, and we have the open data to do it.
6. **Copyright-clean by design** — notes anchor to IDs; licensed text fetched, optional.
7. **Zero-friction onboarding** — bundle a PD Bible natively; no Ruby/Perl/regex cleanup.
8. **Name disambiguation** — James-the-book vs James-the-apostle as distinct typed
   entities in the data model, not user-invented `- Book` suffix hacks.

---

## 6. How it maps onto this codebase

Architecture pattern (from `AGENTS.md`): feature trio `proto / crdt / db / ui / facade`,
Loro is canonical, cross-feature refs via proto `Option<Uuid>`, architect-ui primitives only,
theme tokens, dumb components.

**Recommended shape — hybrid, three pieces:**

1. **`scripture` (bundled reference data, mostly read-only).** New crate(s) for the
   verse store, tagged text, lexicons, cross-refs, versification. Most of this is *not*
   user-mutable domain data, so it can be **bundled assets + an index**, not heavy CRDT.
   - Reader UI: new route `feature_routes/scripture.rs`, mounted in `crates/ui` shell.
   - Reuse/extend the markdown renderer model in `features/task/task-ui/src/markdown.rs`
     (`MdBlock`/`MdInline`) for verse rendering, or a purpose-built verse component.
   - Permalinks + verse-block IDs feed the existing `BlockIndex`.

2. **Annotations — hybrid.** Structured anchors (`verse range`, `wordSpan`, category
   enum, multi-anchor list) as a small CRDT feature trio (`bible-notes-proto/crdt/db`);
   the prose body lives as **vault markdown** so it's raw, LLM-readable, and shareable.
   Cross-feature link to a verse = `Option<Uuid>` verse-block ref via proto.

3. **Entities → wiki.** Bible people/places/events are **wiki pages** under
   `<vault>/Wiki/Entities/`. Seed an ingest from STEP TIPNR via the existing wiki ingest
   pipeline (`wiki-proto` two-step: `enqueue_ingest` → `record_analysis` → `record_pages`).
   Entity-tagging the running scripture text links into these pages — reuse the wikilink
   parser in `features/vault/vault-obsidian/src/obsidian_parse.rs`
   (`[[Page]]`, `[[Page#^block]]`, `((uuid))`).

**Linking/graph reuse:** verse refs and word refs extend the existing ref kinds; the
`VaultGraph` backlinks surface "every note touching this verse." The 4-signal relevance
scorer (`features/wiki/wiki-graph`) can score passage↔note↔entity relevance.

**Text editing caveat:** collaborative prose editing is blocked on the
`loro-text-editor-upgrade` work (`Block.content: String` LWW loses concurrent edits).
For v1, annotations can be vault-markdown edited through the existing vault path; don't
build on per-keystroke string CRDT writes until that upgrade lands.

---

## 7. Suggested build order (slices for agents)

1. **Data spine.** Ingest WEB + BSB (USFM → USJ), assign verse IDs (OSIS + BBCCCVVV),
   bundle versification map. Verify a verse resolves and renders. *(no UI yet)*
   - ✅ **Started (2026-06-16).** New `features/scripture/` crates:
     `scripture-proto` (wasm-clean keystone — 66-book canon, `VerseId` with OSIS +
     BBCCCVVV keys, human/OSIS reference parsing, translation/licensing registry) and
     `scripture` (native USFM ingest → in-memory `Bible` store). `John 3:16` resolves
     end-to-end to clean text. WEB USFM carries per-word `\w …|strong="G…"\w*` tags —
     preserved on install, surfaced in slice 4 (`// FUTURE` in `usfm.rs`).
   - ✅ **Corpus out of the repo (2026-06-16).** Per the Resources Library decision, the
     corpus is not bundled. `scripture::install_usfm_dir` normalizes a source USFM dir
     (noisy `73-JHNengwebp.usfm` names, front matter) into a clean `<TX>/<BOOK>.usfm`
     translation folder; `Bible::load_dir` loads it. Tests use a tiny inline fixture.
     19 tests pass, clippy clean.
   - ✅ **Resources Library tier + corpus installed (2026-06-16).**
     `OrgRoot::resources_dir()` / `bible_dir(tx)` establish the tier (sibling of `vault/`
     + `wiki/`). Full **WEB** (66 books, 31,098 verses) and **BSB** (66 books, 31,086
     verses) installed into `~/.task/orgs/codywright/resources/bible/<TX>/` via the
     `scripture` `install_bible` example — ~37MB on disk, **not** in the repo. Both
     resolve `John 3:16`; cross-translation comparison works (same `VerseId`, different
     text). Per-org placement (rides per-org sync); sync wiring for `resources/` is a
     later step.
   - ⬜ **Slice-1 leftover (deferred):** versification map. WEB + BSB share English
     versification so they align directly today; the Copenhagen/TVTMS map is only needed
     once we add a differently-numbered text (LXX/Hebrew/Vulgate) — do it then.
2. **Reader + permalinks.** Chapter reader route, verse blocks, `[[John 3:16]]` linking,
   backlinks per verse via `BlockIndex`.
   - ✅ **Reader shipped (2026-06-16).** `ScriptureService` (`#[architect::rpc]`,
     plain Facet DTOs) in `scripture-proto`; `scripture::Store` backend serves
     translations/chapter/verse from the resource library; mounted per-org in
     `apps/server`. `/scripture` UI page (`crates/ui`): translation + book pickers,
     prev/next chapter nav, verse list. Each verse renders with its OSIS id as the
     element `id` (permalink anchor). Both verify gates pass (task-ui native +
     task-app-web wasm); 22 scripture tests pass.
   - ✅ **Vault→verse backlinks + ranges (2026-06-16).** Notes link a verse with a
     normal wikilink (`[[John 3:16]]`); the reader surfaces every note that touches a
     verse — no per-verse files. `VerseRange` parses single verses and spans:
     `[[John 3:16-20]]` (same chapter), `[[John 3:20-7:26]]` (cross-chapter),
     `[[Genesis 4:3-Exodus 15:17]]` (cross-book). A span backlinks every verse it
     covers (overlap computed at query time). `ScriptureService::chapter_backlinks`
     scans the org vault (`Store::with_vault`); reader shows a per-verse link count +
     click-through to the linking notes. Verified end-to-end on the real vault.
   - ⬜ **Remaining:** forward rendering (`[[John 3:16]]` in a *note* renders as a verse
     link/hover-preview — the vault editor side); cached/watched backlink index instead
     of per-query scan; click-a-verse in the reader to copy a `[[ref]]`; remember
     last-read position. (Note: backlinks use a vault scan, not `BlockIndex` — verses
     aren't vault blocks; the scan keys by `VerseId`, which is the right model here.)
3. **Translation layer.** Swap + side-by-side parallel; wire ESV API behind a user key.
   - ✅ **Comparison + translation-qualified refs (2026-06-16).** Reader has a Compare
     panel (reference + translations → parallel verse×translation table), backed by
     `ScriptureService::compare`. References can pin an edition: `[[John 3:16@ESV]]`
     (`ScriptureRef`). A markdown `compare` fenced block declares a comparison
     (`extract_compare_specs` / `CompareSpec`) for a future inline renderer. Works
     across single verses and cross-book ranges; verified on WEB + BSB.
   - ✅ **ESV + NIV via API (2026-06-16).** `scripture::api` adds `Provider {Esv,
     ApiBible{bible_id}}` + `ApiTranslation`; the store routes bundled→memory,
     API→live HTTP fetch (`chapter`/`verse`/`compare` are now async). Nothing persisted
     (stays inside ESV/API.Bible caching terms). Keys from env at server start:
     `TASK_ESV_API_KEY` (ESV); `TASK_API_BIBLE_KEY` + `TASK_API_BIBLE_NIV_ID` (NIV via
     API.Bible — only works if the key has NIV access). Verse parsing unit-tested; live
     fetch not yet exercised (no key in this env).
   - ⬜ **Remaining:** confirm the NIV `bible_id` + live-fetch once a key is available;
     inline rendering of `compare` blocks inside notes; versification mapping (once a
     differently-numbered edition is added); optional small bounded cache for API reads.
4. **Strong's + lexicons.** Bundle STEP TAGNT/TAHOT + Strong's/BDB/Thayer's; click-word →
   lexicon page; every-occurrence concordance (backlinks query).
   - ✅ **Phase 1 — foundation on existing tags (2026-06-16).** `usfm::extract_words`
     captures the per-word `strong=` tags already in WEB/BSB; `Bible` stores words + a
     concordance index. Bundled the **public-domain OpenScriptures Strong's lexicon**
     (5,523 Greek + 8,674 Hebrew) at `<org>/resources/lexicon/strongs/{greek,hebrew}.json`.
     Service: `lexicon(code)`, `word_study(tx, ref)` (per-word lemma/translit/gloss),
     `occurrences(code, tx, limit)` concordance. Verified on real WEB — John 3:16 parses
     word-by-word with Greek lemmas. **Known data gap:** eBible WEB/BSB tagging is
     partial (e.g. "love" G25/G26 is untagged) — Phase 2 fixes coverage.
   - ✅ **Phase 2 — STEPBible (2026-06-16).** Unified original-language schema
     `OrigWord {word, translit, lemma, strong, morph, gloss}`; an edition is
     `<org>/resources/original/<ID>/text.jsonl` + `meta.json` (`OrigText` load/serialize).
     `stepbible::parse_tagnt_rows`/`parse_tahot_rows`. Installed **TAGNT** (Greek NT,
     7,948 v / 141k words) + **TAHOT** (Hebrew OT, 21,178 v / 283k words). Complete
     tagging — John 3:16 now has ἠγάπησεν (G0025) with full morphology.
   - ✅ **Phase 3 — SBLGNT + OSHB (2026-06-16).** `morphgnt::parse_morphgnt_rows` (SBL
     Greek NT, lemma+morph, no Strong's) and `oshb::parse_oshb_xml` (Westminster Hebrew,
     OSIS XML via `roxmltree`; Strong's from the lemma attr). Installed **SBLGNT**
     (7,927 v) + **OSHB** (23,213 v). All four editions ~97MB, one schema, not in repo.
   - ✅ **Wired into the service (2026-06-16).** `lexicon::normalize_strongs`
     (`G0025`→`G25`, `H0376G`→`H376`) bridges STEPBible/OSHB codes to the OpenScriptures
     lexicon; `Lexicon::get` accepts source-faithful forms. `Store` loads editions lazily
     per-edition (big files) + caches. `ScriptureService::original_editions()` lists them;
     `interlinear(edition, ref)` returns the per-word breakdown, filling lemma/translit/
     gloss from the lexicon where the edition lacks them (OSHB glosses now resolve).
     Verified on real TAGNT + OSHB. Mounted in the server (`with_originals_root`).
   - ⬜ **Remaining:** reconcile versification between editions (OSHB 23k vs TAHOT 21k —
     needs the Copenhagen/TVTMS map); word-study **UI** (clickable words →
     lexicon/occurrences, interlinear toggle); optional memory tuning (offset-index the
     JSONL instead of full per-edition load).
5. **Annotations.** Typed (NET) + two-level (ESV) anchoring; vault-markdown bodies.
6. **TSK cross-references.** Phrase-keyed, target text inlined.
7. **Entities.** TIPNR → wiki ingest; entity-tag the text; Factbook pages.
8. **Synthesis.** Topic/question notes, Passage Guide panel.
9. *(later)* Atlas, timeline, morphology search, sense lexicon.

Pillars to nail in the first cut (≈80% of the value on free, redistributable,
markdown-friendly data): **typed annotations + two-level verse anchoring + TSK
phrase-keyed cross-refs + Strong's click-through**, on a **stable verse/word ID spine**
with **translation-as-a-swappable-layer**.

---

## 8. Decisions & open questions

**Decided (2026-06-16):**
- **Scripture is read-only.** Users cannot edit Bible text; only system tooling writes it.
- **NIV is a priority translation**, fetched via API (never bundled — copyright). Verify
  the licensing path (API.Bible vs Biblia vs direct Biblica) before wiring — see §3.

**Open:**
- **NIV license path** — confirm which API actually serves NIV and on what terms.
- **Versification source:** Copenhagen Alliance JSON vs STEP TVTMS — pick one, normalize.
- **Verse store as bundled assets vs a real feature trio** — it's read-only, so lean to
  **bundled + indexed** (not heavy CRDT); annotations/entities = CRDT/vault.
- **MACULA depth:** ship syntax trees in v1, or just lemma+morph+Strong's and add trees
  later? (Lean: defer trees.)
- **Sub-verse anchor encoding** in markdown — how do we serialize a `wordSpan` so it
  round-trips and stays LLM-readable?
- **Graph separation** — explicit "scripture layer" flag so the spine doesn't flood the
  thought graph.

---

## 9. Key sources

- Open data: STEPBible-Data (CC BY 4.0 TSV), OpenScriptures morphhb, MACULA (Clear Bible),
  eBible.org (USFM), bereanbible.com (BSB CC0), openscriptures/strongs, Copenhagen-Alliance
  versification-specification, labs.bible.org (NET API), `usfm-grammar` (Bridgeconn).
- Feature references: Logos (Factbook/Bible Word Study/reverse interlinear), Accordance
  (construct search), Blue Letter Bible + BibleHub (Strong's/interlinear/TSK), STEP Bible,
  NET Bible (typed notes), ESV Study Bible (two-level apparatus).
- Obsidian/Logseq prior art: tim-hub/obsidian-bible-reference, kuchejak/bible-linker,
  pmbauer/av-obsidian, selfire1/BibleGateway-to-Obsidian, gslogimaker/my-bible,
  echokos/logseq-berean-standard-bible, Evan Travers "Connected Bible Study in Markdown",
  Biblically Connected (Joschua), faithbasedproductivity.com (backlink-granularity).
