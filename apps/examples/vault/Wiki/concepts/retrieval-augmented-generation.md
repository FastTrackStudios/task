---
type: concept
title: Retrieval-Augmented Generation
created: 2026-05-22
updated: 2026-05-22
tags: [llm, retrieval, architecture]
related: [llm-wiki, source, knowledge-graph]
sources: [karpathy-llm-wiki.md]
---
# Retrieval-Augmented Generation

Retrieval-Augmented Generation, often abbreviated RAG, is an LLM architecture in which relevant source chunks are retrieved at query time and supplied to a model as context for generating an answer.

In the LLM Wiki source, RAG is the main baseline contrasted with a persistent wiki. The critique is that raw-source retrieval repeatedly reconstructs context for each query. When a question requires connecting several documents, the system must retrieve, rank, interpret, and synthesize fragments again instead of reusing prior synthesis.

The LLM Wiki approach does not eliminate retrieval. It changes the retrieval target. Instead of retrieving only raw chunks, the system retrieves from curated wiki pages that already contain summaries, cross-references, filed answers, contradictions, and source links.

This distinction matters for local-first PKM systems. Raw documents remain the immutable evidence layer, while generated wiki pages become an accumulated reference layer. Query-time retrieval then operates over a maintained knowledge base rather than an unprocessed document pile.

RAG remains useful when source fidelity, broad recall, or direct citation to primary documents is required. A maintained wiki is most useful when the goal is to preserve synthesis and make future questions cheaper to answer.
