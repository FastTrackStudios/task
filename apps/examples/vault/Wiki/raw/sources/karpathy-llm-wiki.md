# LLM Wiki — design note

*Source: Andrej Karpathy, 2026.
[gist.github.com/karpathy/442a6bf555914893e9891c11519de94f](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)*

## Core idea

Most uses of LLMs against personal documents look like RAG:
upload a pile of files, the model retrieves chunks at query
time, and synthesizes an answer. The problem is that the LLM
**rediscovers** knowledge from scratch on every question — nothing
accumulates. A subtle question that touches five documents
requires the model to re-find and re-piece-together the relevant
fragments every time. There's no compounding artifact.

LLM Wiki proposes the opposite: instead of retrieving from raw
documents at query time, an LLM **incrementally builds and
maintains a persistent wiki** — a structured, interlinked
collection of markdown files that sits between the user and the
raw sources.

When a new source arrives, the LLM doesn't merely index it for
later. It reads, extracts key information, and integrates the
new material into the existing wiki — updating entity pages,
revising topic summaries, noting contradictions, strengthening
or challenging the evolving synthesis. The knowledge is
**compiled once and kept current**, not re-derived on every
query.

## Three layers

| Layer       | Owner | Description                                                |
|-------------|-------|------------------------------------------------------------|
| Raw sources | User  | Immutable inputs (PDFs, articles, transcripts, notes).     |
| Wiki        | LLM   | Generated markdown pages, indexes, logs, cross-references. |
| Schema      | Both  | A `schema.md` / `purpose.md` contract defining conventions.|

The user is in charge of *sourcing*, *exploration*, and asking
the right questions. The LLM does the *grunt work* — summarizing,
cross-referencing, filing, and bookkeeping. In practice the user
keeps the agent open on one side and Obsidian on the other:
the LLM edits based on the conversation; the user browses results
in real time. *Obsidian is the IDE; the LLM is the programmer;
the wiki is the codebase.*

## Three operations

- **Ingest** — drop a new source into the raw collection. The
  LLM reads it, discusses takeaways with the user, writes a
  summary page, updates the index, revises related entity and
  concept pages, appends a log entry. A single source might
  touch 10–15 wiki pages.
- **Query** — ask a question against the wiki. The LLM searches
  for relevant pages, reads them, and synthesizes an answer with
  citations. Important: **good answers can be filed back into the
  wiki as new pages**. Explorations compound just like ingested
  sources do.
- **Lint** — periodically health-check the wiki: contradictions
  between pages, stale claims, orphan pages, missing
  cross-references, data gaps.

## Index and log

Two files help navigate the wiki as it grows:

- **`index.md`** is *content-oriented*. A catalog of everything
  in the wiki, organized by category, listing each page with a
  one-line summary. The LLM updates it on every ingest. When
  answering a query, the LLM reads the index first.
- **`log.md`** is *chronological*. Append-only entries of ingests,
  queries, and lint passes. If every entry starts with
  `## [YYYY-MM-DD] <op> | <title>`, the log becomes grep-able with
  `grep "^## \[" log.md | tail -5`.

## Why it works

The wiki is a **persistent, compounding artifact**. Cross-references
are already in place. Contradictions have already been flagged. The
synthesis reflects everything you've read. Every new source makes
the wiki richer. Every query, if you file the answer, makes it
richer too.

It applies anywhere knowledge accumulates over time: personal goals
and health, research deep-dives, reading a book chapter by chapter,
team wikis fed by chat logs, due diligence, trip planning, course
notes, hobby specialization.

## Related

- [[Zettelkasten]] — same atomic-note ethos, applied at a
  smaller granularity by a human curator rather than an LLM.
- [[Spaced repetition]] — for memorizing what the wiki teaches.
- [[Knowledge graph]] — what the wiki's wikilink structure
  becomes when you visualize it.
