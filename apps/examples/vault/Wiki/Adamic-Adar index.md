---
title: Adamic-Adar index
type: concept
tags: [graph, algorithm, similarity]
sources: ["adamic-adar-2003"]
folder: "[[Wiki]]"
---

# Adamic-Adar index

Link-prediction score for a pair of nodes: sum over their shared neighbors of
`1 / log(degree(n))`. High-degree shared neighbors are discounted (everyone
links to them, so they're weak signal); rare shared neighbors are stronger.

Used as the **common-neighbor signal** in [[Knowledge graph]] relevance —
weight 1.5 in the [[LLM Wiki]] model.

Related: [[Louvain community detection]], [[PageRank]].
