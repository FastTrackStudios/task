---
name: llm-wiki
description: Drive one of an org's named wikis end to end from the CLI — scaffold it from a purpose statement, write and read pages, ingest sources, drain the review queue, resolve lint findings, propose research, take Edit Requests, and subscribe to other wikis (scripture included). Every verb names its wiki with `--wiki <slug>` or `TASK_WIKI`; nothing assumes one.
runs_as: an org member holding Editor on the wiki (curator), or the developer's own account
trigger: "set up a wiki for X", "scaffold a wiki", "ingest this into the <slug> wiki", "drain the wiki review queue", "what does the wiki say about", "add a page to the wiki", "propose a change to the wiki"
---

# LLM wiki — one named wiki, end to end

An org holds a **set** of wikis (`wiki.many.set`): the long-standing
`knowledge` tier at `<org>/wiki/Knowledge/` and any number of created
ones at `<org>/wikis/<slug>/`. Each is a Karpathy-style LLM-maintained
knowledge base with the same shape — `purpose.md`, `schema.md`,
`index.md`, `log.md`, `raw/sources/`, `_state/` — and the agent's hand
on any of them is `task wiki <verb> --wiki <slug>`.

**No verb assumes a wiki.** `--wiki <slug>` is required, or comes from
`TASK_WIKI`; an omitted flag is a usage error, never "the default one".
The old `--wiki-id` spelling is a hidden alias for one release. A wiki
verb that does not take `--wiki` is one that takes the slug as its
first positional (`page`, `describe`, `set-title`, `set-visibility`,
`refresh-source`, `changes`, `push`) or one that is about the set
(`list`, `create`, `scaffold`).

```bash
export TASK_WIKI=bible-study        # or pass --wiki bible-study on every verb
task wiki list                      # the org's wikis: slug, visibility, page count, title
task wiki describe bible-study      # title, visibility, editors, purpose, root
```

## Mental model

```
   purpose.md ─┐                        ┌─ review queue  (_state/review.json)      ◀── gate
   schema.md  ─┼─ read on every ingest ─┤
   Goals.md   ─┘                        └─ lint findings (_state/lint_findings.json)

   raw/sources/<file> ─ analyze ─ generate ─ FILE blocks → Topics/*, Sources/*, …
                                            REVIEW blocks → review queue
```

Two documents steer the agent (`purpose.md`: what belongs; `schema.md`:
what a page looks like) and one steers the curator (`Goals.md`: what
to write next). Two queues the curator works: the **review queue**
(gated — apply or leave LLM proposals) and **lint findings** (passive —
resolve as you go).

## 0. Scaffold a wiki from a purpose

`wiki create` adds a wiki to the set with a one-paragraph purpose and
the generic schema. `wiki scaffold` writes the contract as well:

```bash
task wiki scaffold \
  --title "Bible Study" \
  --purpose "Notes and questions from a weekly study of the Gospel of John." \
  --visibility unlisted \
  --types topic,question,person \
  --goals-file goals.md            # optional; a starter list otherwise
```

What it does, in order, printing a line per step:

1. `create_wiki` if the slug (derived from the title, or `--slug`) is
   not in the set — the caller becomes the first Editor.
2. `purpose.md` — a document written around the sentence: what it is
   for, who reads it (explains the visibility), the questions it
   answers (one per type), out of scope, how it grows.
3. `schema.md` — a `type:` table from `--types` (`source` is always
   included) with each type's directory (`Topics/`, `Questions/`,
   `People/`, `Sources/`), the required frontmatter, the wikilink rules
   (`[[Page]]`, `[[slug::Page]]`, `[[bible::John.3.16]]`), catalog and
   log conventions.
4. `Goals.md` — from `--goals-file` verbatim, or a starter list that
   names the first page of each type.
5. `catalog rebuild`.

Idempotent: re-running keeps the wiki, keeps a purpose or schema that
is no longer a stub, keeps an existing `Goals.md`, and says `kept` for
each. Edit any of the three afterwards with `task wiki schema
write-purpose|write-schema <file> --wiki <slug>` or `task wiki page
write <slug> Goals.md --from goals.md`.

Example output:

```
wiki    created `bible-study` (Bible Study, unlisted)
wrote   purpose.md
wrote   schema.md (topic, question, person, source)
wrote   Goals.md
rebuilt index.md

next:   task wiki page list bible-study   ·   task wiki ingest --wiki bible-study --source <file>
```

Pre-flight on an existing wiki:

```bash
task wiki schema health --wiki <slug>       # bootstrap_done, schema/purpose present, counts, queue depth
task wiki schema bootstrap --wiki <slug>    # only if bootstrap_done is false; idempotent
```

## 1. Pages — read and write

The authoring surface, over the Pages service (the org router's gates
apply; a wiki that has declared Editors refuses a direct write from
anyone else and points at Edit Requests, §6):

```bash
task wiki page list <slug>                                # path, type, title
task wiki page read <slug> Topics/Signs.md                # markdown, frontmatter included
task wiki page read <slug> Topics/Signs.md --sha          # content hash for a guarded write
task wiki page write <slug> Topics/Signs.md --from signs.md
task wiki page write <slug> Topics/Signs.md --from - --base-sha256 <sha>   # stdin; refused if the page moved
```

A page you write carries the schema's frontmatter. Pages the agent
writes are stamped `ai_generated: true`; add `generated_by: <model>` so
the badge names the producer. Personal notes, tasks and journals go in
the vault, not a wiki.

```markdown
---
title: The seven signs
type: topic
tags: [john, structure]
sources: ["raw/sources/carson-john.md"]
created: 2026-09-02
---
```

## 2. Ingest sources

Three ways a source lands under `raw/sources/`, all SHA-256 deduplicated
(re-importing identical bytes returns the existing ref):

```bash
# A URL or local file, routed by content type (article → readability,
# Google Doc → markdown export, YouTube/podcast → transcript), then
# enqueued for ingest. Over the wire.
task wiki archive https://example.org/essay --wiki <slug>
task wiki archive ./paper.pdf --wiki <slug> --title "Paper" --no-enqueue

# A directory of files, over the wire; enqueue afterwards.
task wiki import --dir ./incoming --wiki <slug>
task wiki rescan --wiki <slug>              # diff only
task wiki rescan --wiki <slug> --enqueue    # and queue ingest tasks

# Run the LLM here (two-step analyze → generate; writes pages, index,
# log, injects `sources:`). FS-only: the wiki must be on this machine.
task wiki ingest --wiki <slug> --source ./paper.md --model gpt-5.4-mini
```

The ingest queue the server holds:

```bash
task wiki ingest-queue list --wiki <slug>
task wiki ingest-queue retry <task-id> --wiki <slug>
task wiki ingest-queue cancel <task-id> --wiki <slug>
task wiki raw list --wiki <slug>            # every raw source
task wiki raw read raw/sources/x.md --wiki <slug>
task wiki raw delete raw/sources/x.md --wiki <slug> -y   # returns the review items raised
task wiki watch on|off|status --wiki <slug> # server-side watcher on raw/sources/
```

### FS-only verbs and where the wiki lives

`ingest`, `deepen`, `lint`, `dedup`, `research`, `context` and
`watch-sources` read a tree directly — they run the LLM here or watch a
local directory — so they need the wiki on this machine. `--wiki <slug>`
asks the server where it lives (`describe` → `root`, e.g.
`wikis/bible-study` or `wiki/Knowledge`) and joins that to the org root
under `TASK_DATA_ROOT`. That works with the embedded backend
(`TASK_EMBED=1`) or a server on the localhost default serving this data
root. Against a remote server the verb is refused and names the verbs
that work over the wire; `--vault <dir>` still names a tree by path
when you have one.

## 3. Drain the review queue (the gate)

LLM-proposed page changes wait here. Do not ship pages without working
through it.

```bash
task wiki review list --wiki <slug>         # id, kind, subjects, message, suggestions
task wiki review apply <item-id> rewrite-page <path> --wiki <slug> - < body.md
task wiki review apply <item-id> append-note  <path> --wiki <slug> - < note.md
task wiki review apply <item-id> research     "<query>" --wiki <slug>
```

`rewrite-page` replaces the page body, `append-note` appends under
`## Edit log`, `research` promotes the item to a research plan. Body
arguments read stdin as `-`; use stdin from an agent to avoid shell
escaping. The item flips `Open → Resolved`.

## 4. Lint, gaps, research

```bash
task wiki lint --wiki <slug> --model gpt-5.4-mini      # one semantic pass (FS-only); persists findings
task wiki lint-findings list --wiki <slug>
task wiki lint-findings resolve <finding-id> resolve|dismiss|promote-review|promote-research --wiki <slug>
task wiki findings --wiki <slug>                       # open findings, either lane

task wiki gaps --wiki <slug>                           # orphans + missing-page wikilinks, no LLM
task wiki clusters --wiki <slug>                       # Louvain communities
task wiki graph --wiki <slug> --json                   # the 4-signal graph
task wiki search "<query>" --wiki <slug>               # TF-IDF (or --hybrid)
task wiki context "<query>" --wiki <slug>              # token-budgeted subgraph as markdown (FS-only)
task wiki dedup --wiki <slug>                          # duplicate groups (FS-only)
task wiki deepen --wiki <slug> --page Topics/Signs.md  # rewrite a thin page (FS-only)
```

When a gap needs outside sources, propose a plan and track it:

```bash
task wiki research --wiki <slug> --gap-kind MissingPage --gap-title "Prologue" --gap-description "…"
task wiki research-plans list --wiki <slug>
task wiki research-plans set-status <plan-id> running|awaiting|integrated|cancelled --wiki <slug>
```

Run the searches with your usual tools, archive what you find (§2),
re-ingest, and set the plan `integrated`.

## 5. Catalog and health

```bash
task wiki catalog show --wiki <slug>        # index.md parsed by type
task wiki catalog rebuild --wiki <slug>     # after hand edits
task wiki health --wiki <slug>              # queue depth, findings, sources, last ingest
task wiki lint-tiers                        # wikilinks that escape their tier (CI-friendly)
```

## 6. Edit Requests — changing a wiki you do not hold Editor on

Once a wiki declares Editors (the creator is the first), direct writes
are theirs alone; everyone else proposes. A request is also an issue on
the org's board.

```bash
task wiki edits open --wiki <slug> --title "Sharpen Signs" --page Topics/Signs.md --file signs.md --summary "…"
task wiki edits list --wiki <slug> [--all]
task wiki edits show <id> --wiki <slug>
task wiki edits diff <id> --wiki <slug>
# Editor:
task wiki edits claim <id> --wiki <slug>    # release to give it back early
task wiki edits accept <id> --wiki <slug>   # lands the change; reject --reason "…" declines it
task wiki edits return <id> --wiki <slug> --reason "…"
task wiki edits editors --wiki <slug>
task wiki edits grant-editor <account> --wiki <slug>    # org admin
task wiki edits gate readers|members|closed --wiki <slug>
```

## 6a. Repo-sourced wikis — the working copy and the push

A wiki created with `--repo <url> [--branch b] [--path docs]` is a
**working copy** of that path in the repository (`describe` shows
`repository:`, `commit:`, and `pending:`/`CONFLICTS:` when they apply).
Edit it exactly like any other wiki — `page write`, accepted Edit
Requests, the app's collaborative editor, a mounted folder — and every
save lands in the wiki, never in the repository. Accepting an Edit
Request writes the working copy and reads `accepted` at once; nothing
is pushed by accepting.

The server re-syncs from upstream on a schedule (`refresh-source` does
it now). A sync updates only pages nobody has touched here; a page
edited here is never overwritten. If upstream changed the same page,
the local version is kept and the page is listed as a **conflict** —
open it, decide what it should say, save it (that clears the conflict),
then push.

When the batch is ready, push it — one branch, one commit with every
local change, one pull request, as *your* forge identity (on GitHub,
your linked account; refused before anything is pushed if you have
none):

```bash
task wiki changes <slug>                       # kind + path per change, base commit, pending PR, conflicts
task wiki push <slug> --title "Clarify setup" [--body "why"]   # prints the PR URL (or the branch)
task wiki changes <slug> --json
```

Pushing again while the first push is unmerged rewrites the same branch
and the same pull request with the fuller change set — there is never a
queue of requests. Once the repository merges it, the next sync sees
the commit, `pending` clears and `changes` is empty: the wiki and the
repository agree. `task wiki push` is refused, with nothing pushed,
when there are no changes, when a conflict stands, or when you are not
an Editor.

## 7. Subscriptions and references

A wiki (or the vault) subscribes to other wikis and resolves references
against them — a subscribed page is read in place, never copied. The
core set every org gets includes scripture, so a study wiki can cite a
verse without holding the text:

- `[[Page]]` — a page in this wiki (bare basename).
- `[[slug::Page]]` — a page in a subscribed wiki of this org.
- `[[domain/slug::Page]]` — the qualified form, one source exactly.
- `[[bible::John.3.16]]` — scripture, `Book.Chapter.Verse`
  (`[[bible::John.3]]` for a chapter). Stamp with `@<date>` to pin a
  version and `#^block` to point at a block.

Subscriptions are managed from the wiki page's **Subscriptions** tab in
the web app (the `Subscriptions` service: list, subscribe, unsubscribe,
refresh, `core_set`, `discover`); there is no CLI verb yet. A wiki's
visibility decides who may subscribe: `public` is listed and open,
`unlisted` admits anyone holding the reference, `private` refuses
outsiders. `task wiki set-visibility <slug> unlisted` changes it.

## 8. MCP tools (when the server exposes them)

The MCP server (`/mcp`, account-scoped) is growing wiki tools that
mirror the verbs above: `list_wikis`, `describe_wiki`, `create_wiki`,
`list_wiki_pages`, `read_wiki_page`, `write_wiki_page`,
`wiki_local_changes` and `wiki_push_changes` (§6a), and the ingest
and review calls after them. Every one takes the wiki's slug; none has
a default. Check `api_reference` for what the connected server exposes;
until they land, `search_vault`/`read_note` reach the vault only and
the CLI is the way into a wiki.

## 9. Cooking: resources vs curated

A `cooking` wiki holds the org's cookbook (`Cookbook/*.cook`, read by
the cookbook and mealplan plugins; the server points the cookbook
service at `<org>/wikis/cooking/Cookbook/` whenever a `cooking` wiki
exists). Two tiers live there, told apart by cooklang metadata, not by
where they sit:

- **Resources** — whole collections imported in bulk so there is a
  massive list to pull from. Each lands in a sub-folder of the
  cookbook, stamped `>> source:` (canonical URL — the identity; a
  re-run skips it), `>> source_site:`, `>> collection:`, `>> author:`,
  `>> imported: YYYY-MM-DD`, `>> tags: resource, <collection-slug>`,
  `>> curated: false`, plus the importer's usual servings / time /
  image URL (the picture is *not* downloaded; attach one with
  `task recipe image` if you want it in the reader).
- **Curated** — the recipes actually cooked. Flip `>> curated: true`,
  add `>> rating: <1-5>` and `>> made: <count>`, and keep the notes and
  variations in a companion page beside the file:
  `Cookbook/<folder>/<slug>.md` with `type: recipe-note`. `Favorites.md`
  in the wiki lists them. A curated recipe is never rewritten by the
  importer, not even with `--refresh`.

```bash
# The Food Wishes collection into Cookbook/Food Wishes/ — idempotent, cron-safe:
task recipe import-collection "https://www.allrecipes.com/recipes/16791/everyday-cooking/special-collections/food-wishes/" \
  --org <slug> --folder "Food Wishes" --author "Chef John" [--limit N] [--dry-run] [--since-file ~/.cache/task/food-wishes.json]
# Bot-protected site: save the listing + pages, then
task recipe import-collection --from-file listing.html --pages-dir pages/ --org <slug>
# Curate one:
task recipe get "Cookbook/Food Wishes/one-egg-shakshuka.cook" --org <slug>   # then edit curated/rating/made
task wiki page write cooking "Cookbook/Food Wishes/one-egg-shakshuka.md" --from note.md
```

Every run ends by regenerating `<Collection>.md` in the wiki (`type:
index`): one line per imported recipe with its source link, curated
ones starred. Hand edits to that page are overwritten — curate in the
companion note. The summary line is `imported N, skipped M present, F
failed (listed)`; a per-recipe failure never aborts the run, a 403/429
from the site stops it with a clear message (exit 1) so nobody gets
hammered.

## Loop summary

```
0. scaffold (once) → schema health
1. archive | import + rescan --enqueue | ingest       (sources in)
2. review list → apply, until 0                       ◀── gate
3. lint → lint-findings resolve; gaps → research      ◀── housekeeping
4. page write / edits open                            (curation)
5. catalog show, health                               (sanity)
```

Repeat 1–3 per batch of sources. Everything is idempotent: SHA dedupe
on sources, `sources:` injection, upsert-by-id on review and finding
state, `scaffold` fills only what is missing.

## Decision boundaries

- Create a wiki, scaffold, ingest, drain queues, resolve findings:
  decide unilaterally.
- Change visibility to `public`, grant Editor, delete a wiki (its slug
  is retired forever): ask the owner first.
- Push a repo-sourced wiki's working copy (`task wiki push`): it opens
  a pull request in someone's repository under your identity — check
  `task wiki changes` first and push when the batch is coherent, not
  after every edit.
- Rewrite `purpose.md` on a wiki you did not scaffold: propose it as an
  Edit Request rather than writing it.
