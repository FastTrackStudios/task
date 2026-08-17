---
type: entity
title: LLM Wiki
created: 2026-05-22
updated: 2026-05-22
tags: [project, llm, knowledge-graph]
related: [retrieval-augmented-generation, wiki-linting, knowledge-graph, wikilink, source, obsidian, zettelkasten]
sources: [karpathy-llm-wiki.md]
---
# LLM Wiki

LLM Wiki is a method for maintaining a persistent markdown knowledge base with LLM assistance. Instead of using an LLM only to answer questions from raw documents, the system asks the LLM to maintain durable wiki pages: source summaries, entity pages, concept pages, comparisons, filed queries, synthesis pages, indexes, and logs.

The central idea is persistent synthesis. Each ingest or query can improve the wiki for future work by preserving summaries, links, distinctions, open questions, and review notes.

## Architecture

The source describes a layered structure:

Raw sources are immutable inputs. They preserve the original material and provide evidence for later claims.

Generated wiki pages are the maintained knowledge layer. They summarize, connect, contrast, and organize information from the raw layer.

Governance files define the contract for maintenance. Examples include schema, purpose, index, and log pages.

This structure makes the wiki a compiled layer between source material and future questions.

## Operations

Ingest reads a new source and updates the wiki. A complete ingest can create a source page, update related entity and concept pages, add index entries, append a log entry, and flag review items.

Query answers questions from the existing wiki and may file reusable answers as query pages.

Lint audits the wiki for contradictions, stale claims, orphan pages, duplicate concepts, missing links, weak citations, and other quality problems.

## Relation to RAG

LLM Wiki is positioned as an alternative to relying only on [[Retrieval-Augmented Generation]] over raw documents. The distinction is not that retrieval disappears. Instead, retrieval shifts toward curated wiki pages that already contain accumulated synthesis.

## Local-first fit

Because the system uses markdown, wikilinks, and source files, it fits local-first PKM workflows. Tools such as [[Obsidian]] can act as an editing and browsing environment, while wikilinks form a navigable [[Knowledge graph]].
