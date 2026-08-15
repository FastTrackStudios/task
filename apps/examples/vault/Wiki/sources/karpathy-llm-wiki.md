---
type: source
title: "Karpathy LLM Wiki"
created: 2026-05-22
updated: 2026-05-22
tags: [llm, wiki, knowledge-management, pkm]
related: [llm-wiki, retrieval-augmented-generation, wiki-linting, knowledge-graph, wikilink, obsidian, source]
sources: [karpathy-llm-wiki.md]
---
# Karpathy LLM Wiki

This source describes a persistent markdown wiki maintained by LLM agents as an alternative to repeatedly answering questions through raw-document retrieval. Its central claim is that knowledge work improves when summaries, cross-references, contradictions, and filed answers become durable artifacts rather than temporary query-time outputs.

The proposed system treats the wiki as a compiled knowledge layer between immutable source documents and future queries. New inputs are ingested into source summaries, entity pages, concept pages, synthesis pages, index updates, and log entries. Later queries can draw from this curated layer instead of reconstructing context from raw source chunks each time.

The source identifies three core operations:

Ingest reads a new source, extracts reusable knowledge, updates related pages, and records the operation in the wiki log.

Query answers questions from the existing wiki and can file reusable answers back into the wiki.

Lint audits the wiki for contradictions, stale claims, duplicate concepts, orphan pages, missing links, and weak citations.

The architecture separates immutable raw sources from generated wiki pages and governance files such as schema, purpose, index, and log. This separation supports source fidelity while allowing the generated layer to evolve through maintenance passes.

The document contrasts this approach with [[Retrieval-Augmented Generation]]. The critique is not that retrieval is useless, but that retrieval over raw chunks does not preserve synthesis. The wiki still relies on search and retrieval, but it retrieves from curated, linked, and source-aware pages.

The design fits local-first PKM systems because it uses markdown, wikilinks, source tracking, and editor-friendly files. [[Obsidian]] is presented as a practical environment for browsing and editing the wiki, while the wikilink structure naturally forms a [[Knowledge graph]].

The source is primarily an architectural argument. It does not provide benchmark data, controlled user studies, or measured comparisons against conventional RAG systems. Its strongest contribution is the operational model for compounding knowledge through persistent synthesis.
