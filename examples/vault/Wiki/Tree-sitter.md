---
title: Tree-sitter
type: entity
tags: [parser, syntax]
sources: ["tree-sitter-docs"]
folder: "[[Wiki]]"
---

# Tree-sitter

Incremental parser generator with error recovery, used for syntax highlighting
in editors. Each language ships its own grammar; queries pull token + node
information out for downstream consumers.

Task's editor uses [[Arborium]] (a Rust wrapper) — see `editor-syntax`. Highlights
markdown, rust, typescript, python, json, bash.

Related: [[Tree-sitter highlighter]], [[Arborium]], [[Sublime syntax]].
