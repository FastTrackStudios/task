---
title: Project Roadmap
tags: [editor, roadmap]
status: in-progress
priority: 2
created: 2026-05-20
folder: "[[Projects]]"
---

# Project Roadmap

Live work list for the editor. Pages in this vault reference the
blocks below via `((uuid))` and `![[Project Roadmap#Heading]]`.

## Goals

Build a markdown editor that:

- Renders Obsidian's flavor in live-preview (callouts, tables, math, embeds).
- Supports Logseq-style block references for cross-page reuse.
- Stores files as plain markdown — no proprietary database.
- Resolves links from the files alone (no separate index).

## Status

Most live-preview rendering is done. Multi-file resolution
works through the `vault` crate. Outline-mode (every line is a
bullet block) is still ahead.

A specific block we want to reference from elsewhere:

The vault is the source of truth at runtime. Disk wins over memory.
id:: 01950000-0000-7000-8000-000000000010

## Open questions

> [!question] Slash menu defaults
> Which commands should live in the top of the menu vs hide behind
> typing? The current order is: Heading 1, Structure, Code, Math,
> Callouts, Link. Open to feedback.

> [!warning] Mermaid renderer
> The vendored fork patches `Instant::now` for wasm32. If you
> upgrade `mermaid-rs-renderer`, redo the patch in
> `external/editor/vendor/mermaid-rs-renderer/`.

## Next slices

1. Multi-file resolution — DONE in `editor + vault`.
2. Wire `task-ui::EditorApp` to consume `vault::Vault`.
3. Outline-mode (Logseq-style bullets) — design pending.
4. Type-aware property editor table view.
