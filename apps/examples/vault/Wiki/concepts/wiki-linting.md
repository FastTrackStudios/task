---
type: concept
title: Wiki linting
created: 2026-05-22
updated: 2026-05-22
tags: [wiki, maintenance, quality]
related: [llm-wiki, source, knowledge-graph, wikilink]
sources: [karpathy-llm-wiki.md]
---
# Wiki linting

Wiki linting is the periodic review of a knowledge base for structural and factual quality problems. In an LLM-maintained wiki, linting is one of the core operations alongside ingest and query.

A lint pass can look for contradictions between pages, stale claims, duplicate concepts, missing source citations, orphan pages, broken wikilinks, weak summaries, and important referenced topics that lack dedicated pages.

Linting is essential because persistent synthesis can compound errors as well as knowledge. If inaccurate summaries or poorly sourced claims are repeatedly reused, later pages may inherit those mistakes. Linting provides a maintenance loop that keeps the wiki useful as an evolving reference layer.

For a local-first markdown wiki, linting can combine mechanical checks with human review. Mechanical checks can detect missing frontmatter, absent source filenames, broken links, duplicate titles, and pages not listed in the index. Human review is still needed for judgment-heavy cases such as conceptual overlap, source interpretation, and contradiction resolution.

In the LLM Wiki model, linting turns the wiki from a passive note archive into an actively maintained knowledge system.
