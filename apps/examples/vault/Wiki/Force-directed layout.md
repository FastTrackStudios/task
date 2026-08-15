---
title: Force-directed layout
type: concept
tags: [graph, visualization, algorithm]
sources: ["fruchterman-reingold-1991"]
folder: "[[Wiki]]"
---

# Force-directed layout

Graph layout algorithm modeling nodes as charged particles and edges as
springs. Run to equilibrium; output is x/y coordinates per node.

[[LLM Wiki]] uses `forceatlas2`; petgraph + a small simulator works for Rust.
Used in [[Knowledge graph]] rendering.

Related: [[Louvain community detection]], [[Fruchterman-Reingold]].
