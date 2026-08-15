---
title: Markdown frontmatter
type: concept
tags: [markdown, metadata]
sources: ["obsidian-docs", "jekyll-frontmatter"]
folder: "[[Wiki]]"
---

# Markdown frontmatter

YAML block at the top of a `.md` file, fenced by `---`. Carries structured
metadata (title, tags, type, sources, …) the renderer + queries consume.

Task's parser is in `editor-state::markdown::parse_frontmatter`. Round-trip
serialization preserves key order via [[Indexmap]].

Related: [[YAML]], [[Wikilink]], [[Properties]].
