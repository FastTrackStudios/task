# Task — Vision

**A local-first, real-time collaborative work-management platform —
backed by markdown files you own.**

> This document is **product vision**: what Task is for and where it's
> going. It deliberately does not describe implementation. For what
> exists today, read [ARCHITECTURE.md](ARCHITECTURE.md); for how to
> work on it, [AGENTS.md](AGENTS.md). If you find a technical claim in
> here, it has drifted — move it or delete it.

## The Problem

Production work — music, video, events, software, creative projects —
involves tasks, schedules, file delivery, team coordination, budgets,
and client communication. These are fragmented across dozens of tools
that don't talk to each other. Generic tools lack domain context;
domain-specific tools don't interoperate. Nothing is self-contained,
nothing works offline, and you don't own your data.

## The Solution

Task manages the full lifecycle of work, on your own terms:

- **Local-first** — every client has a full copy of the data. Works
  offline. Syncs when connected. No server dependency for core
  operations.
- **Real-time collaborative** — multiple people edit the same project
  simultaneously; changes propagate as they happen.
- **File-based source of truth** — plain `.md` files with YAML
  properties and a markdown body. Readable by any text editor,
  portable to any system, greppable forever.
- **Obsidian-compatible** — the vault *is* an Obsidian vault.
  Properties, wikilinks, checkboxes, `.base` views — all native. Task
  enhances the vault; it never replaces it.
- **Multiple views, same data** — desktop, web, mobile, watch, CLI,
  Obsidian, and anything else that can read a folder of markdown.
- **Events as first-class entities** — concerts, services, recording
  sessions, shoots. Recurring events with templates and per-instance
  overrides. Not everything is a project.
- **Self-hosted, privacy-first** — your infrastructure, your data. No
  cloud dependency, no telemetry, no lock-in.

## Core Principles

### 1. Files are the source of truth

Every entity is a `.md` file. Every project is a folder. No
proprietary database, no vendor lock-in. Copy a project folder to a
USB drive and everything goes with it.

```
Projects/Montreal Album/
├── project.md              ← project properties (YAML frontmatter)
├── tasks/
│   ├── Mix track 1.md      ← assignee, due date, priority, subtasks
│   └── Master all.md
├── sessions/               ← DAW projects, stems
├── audio/                  ← bounces, raw recordings
└── deliverables/           ← final exports
```

Delete the server, keep the folders.

### 2. Open standards

YAML frontmatter (Obsidian, Hugo, Jekyll), Markdown, and — where we
integrate with the calendar world — iCalendar/RFC 5545 semantics. Not
"compatible with" a format; actually *being* the format.

### 3. Generic core, specific edges

The engine is universal: tasks, projects, events, schedules, time,
people, files. Domain knowledge — music production stages, training
periodization, practice routines, study plans — lives in **workflows**
layered on top, never hardcoded into the core.

### 4. Events ≠ Projects

A weekly church service is an event that recurs, not a new project
every week. Events carry setlists, stage plots, input lists,
personnel, run-of-show, and venue advance; they have templates with
per-instance overrides.

### 5. Properties are simple, body is rich

YAML frontmatter for key/value metadata. Structured markdown (tables,
checklists) in the body for the complex stuff — setlists, input lists,
notes. Simple data stays simple; rich data stays readable.

### 6. Segregation with unity

Work tasks, personal tasks, and domain-specific tasks stay in their
lanes unless you choose to view them together. Different orgs, one
identity, one app.

### 7. Incremental adoption

Start with a folder of markdown and the CLI. Add the app when you want
views. Add a server when you want real-time collaboration and sharing.
Each layer is optional, and removing one leaves the files intact.

### 8. AI agents are first-class team members

Task is designed for a world where AI agents work alongside humans —
not as a bolted-on API, but as a core assumption.

- Agents need **structured data**. YAML frontmatter is trivially
  parseable; markdown bodies are trivially readable. No scraping.
- Agents need **the same API as humans**. If a person can do it in the
  UI, a bot can do it over RPC. No admin-only actions that bypass the
  service layer.
- Agents need **identity**. A bot is an account with a name and scoped
  permissions. When a bot comments, it shows up as itself.
- Agents need **context**. The folder *is* the context — an agent can
  read an entire project by scanning a directory.
- Agents need **accountability**. Bot actions appear in the same
  activity trail as human actions.

What this looks like: an agent that watches for new mixes and comments
with loudness readings at problem spots; one that pings assignees on
overdue work and writes the weekly summary; one that transcribes vocal
takes into the writing workflow; one that validates deliverables
against a spec and files a task when they don't match.

Design implications: comments are **typed**, not just free text (a
timecode-ranged comment renders as a waveform marker). Actions are
**idempotent**, so bots can retry safely. Everything is addressable.

## Where it's going

### Workflows

Workflows define the stages, checklists, and process for a *type* of
work — reusable templates that give tasks and projects domain-specific
shape.

- **Custom workflows** — define your own for personal or team use.
- **Community workflows** — shared, versioned definitions published by
  others.
- **Composable** — a music-release workflow can embed a mixing
  workflow.

### Domain packs

Each brings its own vocabulary and lifecycle while staying
interoperable with the generic core:

- **Fast Track Studio (music production)** — ideation → demo →
  arrangement → tracking → mixing → mastering → distribution →
  promotion, with session time tracked against tasks.
- **Live events** — setlists, stage plots, input lists, personnel with
  acceptance status, run of show, changeover plans, venue advance,
  budget.
- **Fitness & training** — plans as projects with recurring workouts,
  exercises as structured sub-items with sets/reps/load, deload weeks
  and rest days as first-class scheduled items.
- **Music practice** — sessions with focus areas (technique,
  repertoire, ear training, theory), pieces as projects with milestone
  stages, per-area time logs.
- **Learning & study** — goals broken into modules, spaced-repetition
  review, certification and exam milestones.
- **Household** — meal planning, recipes, pantry, shopping.

### Deliverables and distribution

- **Versioned outputs** — v1 rough → v2 with fixes → v3 approved, with
  an explicit approval lifecycle and timestamped feedback (timecoded,
  for audio and video).
- **Download portals** — role-based file distribution. One link for
  the orchestra; each player picks their part and gets their bundle,
  with shared files (schedule, venue info) automatically included and
  recipient tracking on top.
- **Client review** — scoped, expiring shares for approval without an
  account.

### Federation

One identity across servers. A server hosts many organizations; orgs
are portable directories you can move between machines. Different
devices sync only the slices they care about — a phone might carry one
project, a workstation all of them.

### External systems

We store references; the other system owns its data. Issue trackers
and forges (GitHub, Forgejo), accounting and invoicing, calendars, and
DAW session files all link in by id rather than being re-implemented.

### "Task Compatible"

An open standard and certification so third-party tools can declare
compatibility. A Task Compatible tool conforms to the entity schema,
respects workflow lifecycle hooks, can push and pull state through the
defined service boundary, and shows up natively in the app alongside
everything else. The goal is an ecosystem where domain-specific tools
all speak one language.

## Known limitations & trade-offs

- **Rename/move can break links.** Paths are convenient identifiers;
  ids are the durable ones. Link by id, cache the path.
- **Query performance at scale.** Scanning thousands of files is slow
  without an index. Indexes must stay disposable — rebuildable from
  the files, never authoritative.
- **No atomic multi-file transactions.** Completing a task and
  updating project progress touches two files. We accept eventual
  consistency.
- **Permissions are coarse where storage is coarse.** Structure
  folders to match permission boundaries.
- **Real-time text co-editing is scoped.** Collaborative editing
  applies to vault markdown files; not every surface in the product is
  a shared document.

These trade-offs are intentional. We optimize for **data ownership,
portability, and offline capability** over strict consistency
guarantees. For work where the products are files and the metadata is
structured, that's the right trade.
