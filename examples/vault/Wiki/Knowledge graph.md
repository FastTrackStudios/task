---
title: Knowledge graph
type: concept
tags: [graph, knowledge-management, visualization]
sources: ["llm-wiki-gist", "obsidian-graph-docs"]
status: in-design
folder: "[[Wiki]]"
---

# Knowledge graph

A graph of pages connected by [[Wikilink]]s, [[Block reference]]s, and
implicit signals (shared tags, shared [[Source]]s, [[Type affinity]]).

Task's graph (planned) cribs from [[LLM Wiki]]'s 4-signal model:

1. **Direct links** (weight 3.0) — `[[A]]` from A to B counts.
2. **Source overlap** (weight 4.0) — shared `sources:` frontmatter.
3. **Adamic-Adar** common neighbors (weight 1.5).
4. **Type affinity** (weight 1.0) — concept↔concept ≠ concept↔source.

Cluster via [[Louvain community detection]]. Render with a force-directed
layout — see [[Force-directed layout]].

Related: [[Backlink]], [[PageRank]], [[Knowledge management]].
