---
title: 2026-05-21 LLM as wiki maintainer
type: fleeting
tags: [llm, wiki, idea]
folder: "[[Inbox]]"
---

# 2026-05-21 LLM as wiki maintainer

Karpathy's llm-wiki pattern is interesting because it inverts
the usual RAG flow — instead of retrieving from raw docs on
every query, the LLM *compiles* the wiki once and maintains it.

Could Task do this? Have a `task wiki ingest <file>` that reads
a source, finds relevant Wiki/ notes, proposes diffs. Probably
needs the [[Knowledge graph]] feature first so the LLM knows
which pages are neighbors.

→ if this hardens, promote to [[Wisdom]] or open `plans/llm-ingest.md`.
