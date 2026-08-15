---
title: LLM Wiki
type: entity
tags: [project, llm, knowledge-graph]
sources: ["karpathy-gist-llm-wiki", "llm-wiki-readme"]
status: research
folder: "[[Wiki]]"
---

# LLM Wiki

Tauri+React desktop app by nashsu, implementing [[Andrej Karpathy]]'s LLM-wiki
pattern. Crib source for Task's [[Knowledge graph]] design.

Key bits worth copying:

- **4-signal relevance**: direct links (3.0), source overlap (4.0),
  Adamic-Adar common neighbors (1.5), [[Type affinity]] (1.0).
- **Louvain community detection** for visual clustering.
- **Cohesion = intra-edges / possible-edges** per community.
- **Graph insights**: surprising connections + knowledge gaps.

Stack (JS side): sigma + graphology + graphology-communities-louvain. Rust
equivalents: petgraph + an in-tree louvain, or skip clustering for v1.

Related: [[Knowledge graph]], [[Louvain community detection]],
[[Adamic-Adar index]], [[Andrej Karpathy]].
