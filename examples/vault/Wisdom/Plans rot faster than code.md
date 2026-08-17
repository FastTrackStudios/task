---
title: Plans rot faster than code
type: claim
tags: [planning, process]
folder: "[[Wisdom]]"
---

# Plans rot faster than code

Architecture docs written in week 1 describe a system that
won't ship; they're built on assumptions you don't have yet.
By week 4, the codebase has answered those questions and the
plan is partial fiction.

Implication: write plans as **decisions + open questions**, not
finished blueprints. Commit messages and code-level comments are
the durable record; plans are scratch paper. See `plans/` vs
`plans/archived/` in this repo — 24/29 plans got archived once
their target architecture changed.

Related: [[Zettelkasten]] (atomic, durable), [[Wiki]] vs [[Wisdom]] split.
