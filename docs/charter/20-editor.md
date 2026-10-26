# The Ideal Editor

_Charter #20 (formerly issue #20)._

The writing surface itself — the thing you spend the most hours inside.

## Hard requirements

_To be detailed._

Prompts worth answering: What is the editing model — live preview, source, or both without a mode switch? What must never be lost, and what is the save contract? How fast must it stay at a 10,000-line note? What does `[[` owe you — completion, creation, disambiguation? Which widgets may render inline, and can they be interactive without stopping it being markdown? What does the file look like when opened in vim or Obsidian — is every construct still plain text? What of vim mode, slash commands, and the keyboard surface is mandatory? How does collaborative editing behave when two people type in the same paragraph, and what happens when one is offline? What does the mobile editing experience have to match?

## Non-goals

_To be detailed._

## Current state

A from-scratch CodeMirror-inspired stack (12 crates): pure `editor-state` with transactions and a markdown live-preview pass, a Dioxus `contenteditable` view, vim mode, syntax, LSP, mermaid and typst. Wikilink decorations render setlist cards, song strips, scripture verses and contact chips inline. Loro CRDT for live multiplayer; 2s autosave debounce with sha-based conflict detection. A widget registry lets features contribute note experiences.

Note the standing constraint: whatever the editor renders, the file on disk stays plain portable markdown.

Related: [The Ideal Vault (PKM)](08-vault.md).
