---
title: Type affinity
type: concept
tags: [graph, algorithm]
sources: ["llm-wiki-readme"]
folder: "[[Wiki]]"
---

# Type affinity

[[LLM Wiki]]'s fourth relevance signal. Each page declares a `type:` in
frontmatter (concept / entity / source / synthesis / query). A 5×5 affinity
matrix weights pairs differently:

- concept ↔ concept = 0.8 (already similar, less surprising)
- concept ↔ synthesis = 1.2 (mid-bridge, high signal)
- source ↔ source = 0.5 (raw inputs rarely interesting together)

Related: [[Knowledge graph]], [[LLM Wiki]].
