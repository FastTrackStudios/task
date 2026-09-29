# Bible Study — schema

The contract between the curator (human) and the maintainer (LLM agent) for the `bible-study` wiki. The agent reads this on every ingest.

## Page types

Every page carries a `type:` frontmatter field and lives in the directory for its type.

| `type:` | Lives in | What |
|---|---|---|
| `passage` | `Passages/` | One chapter or pericope: its setting, how it has been read, and where it connects. Anchors to verses (`anchors:`) rather than quoting them. |
| `topic` | `Topics/` | One subject the wiki covers, synthesised across its sources. |
| `question` | `Questions/` | A question the wiki set out to answer: the answer, and the citations behind it. |
| `person` | `People/` | A person: who they are, and what they said or did that matters here. |
| `source` | `Sources/` | A summary of one imported document under `raw/sources/`. |

Pages outside this wiki use other types (`task`, `daily`, `meeting`, …) — those are not wiki pages.

## Required frontmatter

```yaml
title: Page title
type: passage              # one of the table above
tags: [comma, separated]   # optional but recommended
sources: ["raw/sources/<file>", ...]  # required for source pages, and for any claim taken from one
created: YYYY-MM-DD
```

Pages the agent writes also carry `ai_generated: true` and `generated_by: <model>`.

## Cross-references

- A page in this wiki: `[[Page title]]` — bare basename, so folder moves don’t break links.
- A page in a wiki this one subscribes to: `[[slug::Page]]`, resolved through the subscription rather than copied here. Scripture is `[[bible::Book.Chapter.Verse]]` — `[[bible::John.3.16|John 3:16]]`.
- Never link out to the vault; the vault links in.

## Catalog + log

- `index.md` is the catalog, organised by `type:`. The agent updates it on every ingest; `task wiki catalog rebuild --wiki bible-study` rebuilds it from the tree.
- `log.md` is append-only. Each entry starts `## [YYYY-MM-DD] <op> | <title>` so `grep '^## \['` gives a clean timeline.
- `purpose.md` says what belongs here; `Goals.md` says what to write next.
