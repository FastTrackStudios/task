---
title: Live preview
type: concept
tags: [editor, ui, markdown]
sources: ["obsidian-live-preview-docs"]
folder: "[[Wiki]]"
---

# Live preview

Editor mode that renders markdown formatting in-place (bold, italic, links,
headings) without switching to a separate preview pane. Source markup stays
visible at the caret; renders fade in elsewhere.

Task's editor implements it via the `markdown::live_preview` decoration pass —
see [[Editor]]. The full plumbing is in `editor-state/src/markdown.rs`.

Related: [[WYSIWYG]], [[Source mode]], [[Obsidian]].
