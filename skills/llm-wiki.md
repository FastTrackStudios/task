---
name: llm-wiki
description: Drive one of a Task org's named wikis end to end — create it from a purpose, declare its schema, write pages, ingest sources, drain the review queue, lint — from the CLI or over MCP. Use when an agent (or human) needs to set up or maintain a wiki; every verb names its wiki, none assumes one.
---

# LLM wiki — named wikis

An org holds a **set** of wikis (`task wiki list`). Each wiki is **one
subject**: a curated tree of markdown pages with frontmatter, governed
by two documents at its root:

- `purpose.md` — why the wiki exists, who reads it, the questions it
  should answer, what is out of scope. Its `title:` frontmatter is
  where the wiki's title lives.
- `schema.md` — what a page looks like: the `type:` vocabulary, the
  required frontmatter, `[[Page title]]` linking, catalog + log rules.

`knowledge` is the org's long-standing default tier; created wikis
live at `<org>/wikis/<slug>/`. Slugs are permanent — a deleted wiki's
slug is retired, never reused — so list before you create.

## The loop (CLI)

`task <verb>` talks to a running server (`TASK_VOX_URL`) as the
account in `.env`; pick the org with `--org <slug>`.

1. **Orient**: `task wiki list`, then `task wiki describe <slug>` for
   visibility, editors and source.
2. **Create**: `task wiki create --title "Music Theory" --purpose "…"
   [--slug music-theory] [--visibility private|unlisted|public]`.
   Private by default. Scaffolds `purpose.md`, a default `schema.md`,
   `index.md`, `log.md`. The caller becomes the first Editor.
3. **Declare the shape**: `task wiki schema show --wiki <slug>` /
   `task wiki schema purpose --wiki <slug>`; replace with
   `task wiki schema write-schema <path|-> --wiki <slug>`.
4. **Fill it**: ingest sources (`task wiki archive <url> --wiki <slug>`,
   `task wiki import --dir … --wiki <slug>` + `task wiki rescan
   --enqueue`, or `task wiki ingest --wiki <slug> --source …` to run the
   LLM here), then gate the proposals: `task wiki review list` →
   `task wiki review apply <id>`.
5. **Keep it honest**: `task wiki lint`, `task wiki lint-findings
   resolve`, `task wiki gaps`, `task wiki research`.
6. **No Editor?** Open an Edit Request instead of writing directly:
   `task wiki edits open --wiki <slug> --title … --page … --file …`.
   Other wikis by reference: `[[bible::John.3.16]]`, `[[slug::Page]]`
   resolve through the vault's or wiki's subscriptions.

## Wikis over MCP

The same loop is available as MCP tools on `POST /mcp` (account lane,
every tool takes an optional `org`) and `POST /org/<slug>/mcp`. They
are thin calls into the org's wiki backends — an MCP write is the same
write the CLI or the web app makes, attributed to the same account
(the static `TASK_MCP_TOKEN` acts as the server itself: no Editor is
recorded, and the Edit lane does not apply to it).

| Tool | Arguments | What it does |
|---|---|---|
| `list_wikis` | — | The org's wikis: slug, title, visibility, purpose, page count, `default`/`repo_sourced`. **Call first; never guess a slug.** |
| `describe_wiki` | `wiki` | Summary + config: editors, `has_edit_lane`, `you_may_write_directly`, proposer gate, repo source. |
| `create_wiki` | `title`, `purpose`, `visibility?` (`private`), `slug?` | Creates and returns the summary. Then `write_wiki_schema`. |
| `list_wiki_pages` | `wiki` | Every page: path, title, `type`, size, modified. Nothing filtered. |
| `read_wiki_page` | `wiki`, `path` | Markdown + `sha256`. Keep the sha to edit. |
| `write_wiki_page` | `wiki`, `path`, `markdown`, `base_sha256?` | Create or replace. With `base_sha256` the write is refused if the page changed since it was read; **without it the write is unconditional** (fine for a new page, destructive on an existing one). Returns the new sha. Refused when the wiki has Editors and you are not one. |
| `read_wiki_schema` / `write_wiki_schema` | `wiki` / `wiki`, `markdown` | `schema.md`, whole-document. |
| `read_wiki_purpose` / `write_wiki_purpose` | `wiki` / `wiki`, `markdown` | `purpose.md`, whole-document — keep the `title:` frontmatter. |
| `search_wiki` | `wiki`, `query`, `limit?` (20) | Token search: ranked paths, titles, snippets. |
| `list_wiki_subscriptions` | `wiki?` | What the org vault (or the named wiki) subscribes to, with local-copy state. |
| `subscribe_wiki` | `qualified_id` (`domain/slug`), `resource?` (false), `wiki?` | Take a source on for the vault (or the named wiki). |

The recipe an agent should follow, in tool calls:

```
list_wikis                                   # is there a wiki for this subject?
create_wiki(title, purpose)                  # if not — private by default
read_wiki_schema(wiki) → write_wiki_schema   # declare page types + frontmatter
read_wiki_purpose(wiki)                      # what the curator cares about
search_wiki(wiki, query) / list_wiki_pages   # find what exists before adding
read_wiki_page(wiki, path)                   # … and its sha
write_wiki_page(wiki, path, markdown, base_sha256)
```

Pages are markdown with YAML frontmatter as `schema.md` prescribes (at
least `title:` and `type:`) and link with `[[Page title]]`. A stale
`base_sha256` comes back as a tool error that says to re-read and
merge; do that rather than dropping the sha. The vox surface behind
these tools (`wiki/registry`, `wiki/pages`, `wiki/schema`,
`wiki/search`, `wiki/subscriptions`, and the `wiki/edits` lane that
has no MCP tool yet) is listed by `api_reference`.
