# Bible Study Library — schema

The contract between the curator (Alice) and the maintainer (an LLM agent) for the `bible-study-library` wiki. The agent reads this on every ingest; `skills/wiki-style.md` says how a page is laid out.

This is the **library**: research, written by an agent, read by a person. When Alice has studied a page enough she takes it into her own **Bible Study** (`bible-study`) — a copy that is hers to rewrite, which remembers it came from here. Nothing in this wiki is her conclusion until it has crossed.

## Page types

Every page carries a `type:` frontmatter field and lives in the directory for its type.

| `type:` | Lives in | What |
|---|---|---|
| `passage` | `Passages/` | One chapter or pericope: its setting, how it has been read, and where it connects. Anchors to verses (`anchors:`) rather than quoting them. |
| `topic` | `Topics/` | One subject the wiki covers, synthesised across its sources. |
| `question` | `Questions/` | A question the wiki set out to answer: the answer, and the citations behind it. |
| `person` | `People/` | A person: who they are, and what they said or did that matters here. |
| `word` | `Words/` | A Hebrew or Greek word: `lemma`, `translit`, `strongs`, `language`, `gloss` in the frontmatter; its word study appears under the page. |
| `source` | `Sources/` | A summary of one archived source under `raw/sources/`, with timestamped notes. |
| `path` | `Paths/` | A study path: an ordered list of pages, each with why it is there. |

Pages outside this wiki use other types (`task`, `daily`, `meeting`, …) — those are not wiki pages.

## Required frontmatter

```yaml
title: Page title
type: passage              # one of the table above
summary: "The page in one sentence."
tags: [comma, separated]   # optional but recommended
sources: ["raw/sources/<file>", ...]  # required for source pages, and for any claim taken from one
created: YYYY-MM-DD
```

Pages the agent writes also carry `ai_generated: true` and `generated_by: <model>`.

## Cross-references

- A page in this wiki: `[[Page title]]` — bare basename, so folder moves don’t break links.
- A page in another wiki: `[[slug::Page]]`. Scripture is `[[bible::Book.Chapter.Verse|Full Name C:V]]`.
- A claim from a video: `[[<source-basename>#^t<seconds>|mm:ss]]`, using only anchors the transcript has.

## Catalog + log

- `index.md` is the catalog, organised by `type:`; `task wiki catalog rebuild --wiki bible-study-library` rebuilds it.
- `log.md` is append-only: `## [YYYY-MM-DD] <op> | <title>`.
- `purpose.md` says what belongs here; `Goals.md` says what to research next.
