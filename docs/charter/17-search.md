# Searchable - Anywhere - Anything

_Charter #17 (formerly issue #17)._

One query surface over everything the system holds — notes, files, file *contents*, email, tasks, events, contacts, wiki, scripture, chat — reachable from anywhere, on any device.

The test: there is no thing in Task you can only find by remembering where you put it.

## Hard requirements

_To be detailed._

Prompts worth answering: What is searchable — names, metadata, full text, transcripts, the inside of PDFs and audio? What is the latency budget, and does it hold at 100k notes and a terabyte of media? Does search work offline, and if so over what subset? How are results ranked across wildly different kinds of thing? What does structured query look like next to fuzzy text — tags, dates, project, person, type? Is search a view you can save, and does a saved search become a folder? Who can search what, given permissions? What happens on a phone versus a desktop?

## Non-goals

_To be detailed._

## Current state

Fragmented. `search_vault` covers notes; the wiki has its own graph queries; files have no search at all (tag lenses are explicitly deferred); email search is whatever the provider offers. There is no unified index and no cross-domain ranking.

Related: [A Single Trusted System](02-single-trusted-system.md), [The Ideal File Manager](03-file-manager.md), [The Ideal Vault (PKM)](08-vault.md).
