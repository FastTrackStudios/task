---
title: Louvain community detection
type: concept
tags: [graph, algorithm, clustering]
sources: ["blondel-2008-louvain"]
status: stable
folder: "[[Wiki]]"
---

# Louvain community detection

Greedy [[Modularity]] optimization that partitions a graph into communities by
repeatedly moving nodes between groups to maximize modularity gain.

Used in [[Knowledge graph]] rendering — clusters become visual groupings, and
**cohesion** (intra-community-edges / possible-edges) becomes a quality score.

`graphology-communities-louvain` is the JS impl in [[LLM Wiki]]; the Rust
equivalent lives in `petgraph`-adjacent crates or as a small in-tree
implementation.

Related: [[Modularity]], [[Adamic-Adar index]], [[PageRank]].
