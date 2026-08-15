---
title: Loro
type: entity
tags: [crdt, library, rust]
sources: ["loro-docs"]
priority: 1
folder: "[[Wiki]]"
---

# Loro

Rust [[CRDT]] library by Loro Dev. Op-based, supports text, lists, maps, and
trees with consistent merges. Wire format is binary updates; snapshots are
periodic full exports.

Used by Task across every doc — see [[Architect]] for the trait surface that
wraps it, and [[Task]] for the product that consumes it.

See also [[Yjs]], [[Automerge]].
