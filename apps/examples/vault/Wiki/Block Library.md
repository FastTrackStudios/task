---
title: Block Library
tags: [reusable, snippets]
created: 2026-05-21
folder: "[[Wiki]]"
---

# Block Library

Reusable building blocks. Reference these from anywhere using
`((uuid))` for an inline chip or `{{embed ((uuid))}}` for an
inline card.

## Principles

Three things to keep in mind when shipping notes software.
id:: 01950000-0000-7000-8000-000000000001

In detail:

- Files on disk are the source of truth.
- Indexes are rebuildable. Don't depend on them.
- Render performance matters more than feature surface area.

This paragraph also has an Obsidian short-id at the end so it
can be referenced via `![[Block Library#^principles]]`. ^principles

## Quotes

A note that no one reads is a note that doesn't exist.
id:: 01950000-0000-7000-8000-000000000002

## Definitions

A vault is a directory of markdown files treated as one corpus.
id:: 01950000-0000-7000-8000-000000000003

A block is anything addressable by an id — paragraph, list
item, heading, or fenced block.
id:: 01950000-0000-7000-8000-000000000004
