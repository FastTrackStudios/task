# Wiki schema

The contract between the curator (human) and the maintainer (LLM agent) for this wiki.

## Page types

Every wiki page carries a `type:` frontmatter field.

| `type:`      | What                                              |
|--------------|---------------------------------------------------|
| `entity`     | A person, place, organization, product, project. |
| `concept`    | An idea, technique, term, pattern.               |
| `source`     | A summary of a single imported document.         |
| `synthesis`  | A cross-cutting view (LLM- or human-authored).   |
| `comparison` | Side-by-side of two or more entities/concepts.   |
| `query`      | A filed answer to a question, with citations.    |

Pages outside `Wiki/` use other types (`task`, `daily`, `meeting`, `fleeting`, `claim`, etc.) — those are not wiki pages.

## Required frontmatter

```yaml
title: Page title
type: concept            # see table above
tags: [comma, separated] # optional but recommended
sources: ["src-id"]      # required for source/synthesis/comparison/query
folder: "[[Wiki]]"       # virt-folder parent
```

## Cross-references

`[[Page title]]` wikilinks. Bare basename; folder moves don't break links.

## Catalog + log

- `Wiki/index.md` — content catalog, organized by `type:`. LLM-maintained.
- `Wiki/log.md` — append-only timeline. Each entry: `## [YYYY-MM-DD] <op> | <title>` so `grep '^## \['` gives a clean history.

## Raw layer

`Wiki/raw/sources/` is the immutable input layer. New documents land via `task wiki import` or `submit_research_result`. The agent reads from there but never rewrites the bytes.

## State

`Wiki/_state/` carries opaque agent state (ingest queue, review queue, lint findings, research plans, snapshot). JSON; agent-managed.
