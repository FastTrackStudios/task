# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

One interface, three shells: the browser app, the desktop app and the iOS
app all render the same Dioxus UI (`crates/ui`) — the desktop and phone
apps are native wrappers around it, not separate native design languages.
Design for the web, at phone and desk widths alike.

## Users

**Primary: production teams** — small studios and production teams doing
music, video and live-event work, and the people they deliver to. A team
lead runs projects, a team does the work, clients are invited into the
specific projects and deliverables that concern them.

The team's work is organised as **orgs** (a studio, a client, a church, a
personal org); one person belongs to several and moves between them. AI
agents are members too, with their own identity and scoped permissions
(VISION.md, principle 8).

## Product Purpose

Task is the one place a production team runs its work: what to do today,
where every project stands, what has been delivered and approved, and
where the time went. It replaces the spread of a todo app, a project
board, a review tool (Frame.io), a file share (Nextcloud) and a notes
app with one system whose data is plain markdown files the team owns.

Success: the team trusts it as the **central todo and project list** —
nothing due is ever hidden, every project's state is visible at a
glance, and a client can review and approve a deliverable without the
team reaching for another tool.

## Positioning

- **The files are the product.** Every task, project and note is a `.md`
  file with YAML frontmatter in a folder the team owns — Obsidian-
  compatible, greppable, portable. Delete the server, keep the folders.
- **Local-first and real-time at once.** Works offline on every device,
  syncs and collaborates live when connected.
- **Built for production work, not generic tasks.** Deliverables with
  versions and timecoded review, sessions and stems as real files,
  events (services, sessions, shoots) as first-class and recurring,
  time tracked against tasks.
- **Self-hosted and federated.** The team's own server; orgs on other
  servers can be reached and subscribed to.

## Operating Context

The same person uses Task in very different situations, and all of them
are real:

- **On a phone, on the move** — quick capture and checking things off
  between sessions, in the car, at a service.
- **At a desk on big screens** — planning, triage and deep work, often on
  large or multiple monitors.
- **In the studio** — beside a DAW, frequently in a dim room, hands busy;
  Task is glanced at, not studied.
- **With clients present** — showing work and reviewing deliverables
  together; the interface is part of how trustworthy the team looks.

The core daily jobs, in order of weight:

1. **Tasks and projects** — capture, plan the day (Today / Upcoming /
   Anytime), finish things, keep every project's state visible.
2. **Client delivery** — versioned deliverables, timecoded review and
   approval (the review platform, `files_ui::review`; the global player
   for audio).
3. **Time and schedule** — timers against tasks, the calendar, events.

Notes, wikis and study (including the Bible Study wikis) exist and are
used, but are not the core jobs for this audience.

## Capabilities and Constraints

- Shipped surfaces: Home, Tasks (list + kanban), Projects (+ project
  detail with parts and deliverables), Inbox, Schedule, Gantt, Timer,
  Goals, Milestones, Files, Vault, Wikis, Contacts, Members, Settings,
  plugin apps (`/app/:app`). Desktop shell: icon rail, a context side
  column, a top bar with capture and timer, a right dock. Phone shell:
  bottom tab bar, floating capture, page action bars.
- Theming: preset themes (light/dark) through `architect-ui`, plus
  per-device appearance (text size, density, corners, contrast, font,
  System mode) in `crates/ui/src/appearance.rs`.
- Multi-org: views can span every org the user selects ("All").
- Every action a person can take is an RPC an agent can take; the UI
  holds no privileged path.
- Deliverable media is never a bare `<video>`/`<audio>` — it plays
  through the review platform or the global player (CLAUDE.md).
- Undecided: pricing, licensing and any hosted offering.

## Brand Commitments

- Name: **Task**, made by **FastTrackStudio**.
- No other voice, identity or asset commitments have been confirmed.

## Evidence on Hand

- The seeded demo world (`examples/studio`, `apps/server/src/example_org.rs`):
  ACME Audio and VNT Video, with a cast of users, projects, tasks,
  audio and video deliverables. It is demo data, not customers.
- No testimonials, customer names, metrics or press exist; do not
  invent any.

## Product Principles

1. **Never hide what is due.** A list, a count and a sidebar must agree,
   and overdue work is the loudest thing on the screen, never folded away.
2. **One glance from anywhere.** What matters now must read at arm's
   length in a dim studio and in a two-second phone check.
3. **Client-ready by default.** Anything a client might see — review,
   deliverables, project state — has to look as trustworthy as the work.
4. **The files stay the truth.** Interface conveniences never invent
   state the markdown doesn't hold.
5. **Same power for people and agents.** No action exists only in the UI.

## Accessibility & Inclusion

No formal standard has been set. Known needs from the operating context:
legibility in dim rooms and at a glance (contrast, text size — the
per-device High contrast and text-size settings exist for this), touch-
sized targets on phones, and one-handed use on the move.
