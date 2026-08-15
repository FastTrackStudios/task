---
title: Editor
type: synthesis
tags: [project, editor]
sources: ["editor-readme", "codemirror-docs"]
status: integrated
priority: 1
folder: "[[Projects]]"
---

# Editor

CodeMirror-style editor crate, now Task-owned (was a subtree from upstream
Editor). Lives at `features/editor/`. Provides `EditorApp` Dioxus component
with [[Live preview]], [[Vim modal editing]], slash menu, [[Tree-sitter]] syntax.

Integrated into [[Task]] via `task-ui::App`.

Related: [[CodeMirror]], [[Live preview]], [[Vim modal editing]], [[Dioxus]].
