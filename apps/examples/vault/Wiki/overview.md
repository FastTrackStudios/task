---
type: synthesis
title: Wiki overview
created: 2026-05-22
updated: 2026-05-22
tags: [overview, wiki, knowledge-management]
related: [llm-wiki, knowledge-graph, crdt, wikilink, retrieval-augmented-generation, wiki-linting]
sources: [karpathy-llm-wiki.md]
---
# Wiki overview

This wiki is a reference knowledge base for the Task project. It covers local-first PKM, CRDT-backed collaboration, markdown knowledge structures, modal editors, graph visualization, and LLM-assisted maintenance workflows.

The core technical themes include [[CRDT]] data models, [[Loro]], local-first synchronization, markdown-based linking, [[Wikilink]] structure, and [[Knowledge graph]] navigation. It also tracks relevant tools such as [[Obsidian]], [[Logseq]], [[Dendron]], [[Dioxus]], [[CodeMirror]], [[Tree-sitter]], [[Nix]], and [[Cargo]].

The wiki includes concepts from personal knowledge management such as [[Zettelkasten]], [[Block reference]], [[Markdown frontmatter]], and [[Spaced repetition]]. These pages help define the vocabulary for building and evaluating Task's local-first knowledge workspace.

The newly ingested LLM Wiki source expands the wiki's own operating model. It frames the wiki as a persistent synthesis layer maintained by LLM agents rather than a passive document collection. In this model, raw sources remain immutable, generated wiki pages preserve reusable knowledge, and governance files such as the index and log keep the system navigable.

The source also clarifies the contrast with [[Retrieval-Augmented Generation]]. Retrieval remains useful, but the preferred target is curated synthesis rather than only raw source chunks. [[Wiki linting]] becomes a necessary maintenance practice because persistent knowledge can compound errors if contradictions, stale claims, weak citations, and duplicate concepts are not reviewed.

Overall, the wiki now covers both the subject matter of Task and the maintenance pattern used to keep that subject matter coherent over time.
