---
title: Block reference
type: concept
tags: [markdown, links]
sources: ["logseq-docs", "obsidian-docs"]
folder: "[[Wiki]]"
---

# Block reference

Stable id attached to a paragraph (or block) so it can be linked or embedded
elsewhere. [[Obsidian]] uses `^block-id` suffixes; [[Logseq]] uses `id::
<uuid>` properties. Task supports both via the `BlockIndex` in
`features/vault/vault/src/blocks.rs`.

Related: [[Wikilink]], [[Block Library]], [[Block embed]].
