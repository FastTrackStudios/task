# Task

**Local-first. Realtime. Collaborative. Multiplayer. Extensible.**

A workspace for building cross-domain apps that *feel native*, work
*offline*, sync *instantly*, and never lock you into a vendor. Every
domain — projects, time tracking, invoicing, inventory, recipes,
agent chat, calendar — is a self-contained feature you can use
together or strip out, all written in Rust + Dioxus.

## The product

Task is organized around a **sub-vault taxonomy** — eight
specialized memory layers, each answering a different question
about your work and life. Sub-vaults are linkable but governed
by **directional reference policies** (Knowledge cannot reach
back into Journal; Records is graph-isolated by design). The
same five-part core (Knowledge / Wisdom / Journal / Inbox /
Records) applies recursively — to a person, a team, a project,
or an LLM agent.

### The sub-vault taxonomy

| Sub-vault | Question it answers | Author | Tag |
|---|---|---|---|
| 📥 **Inbox** | What's unprocessed? | Capturer (human / bridge) | `fleeting` |
| 📓 **Journal** | What happened? | Humans + LLM teams | `journal` |
| 🧠 **Wisdom** | What do I understand? | Humans (LLMs draft-assist) | `atomic` |
| 📚 **Knowledge** | What is true? | LLMs (humans review) | `knowledge` |
| 🔒 **Records** | What must stay private? | Humans only | `records` |
| 📁 **Projects** | What am I building? | Humans + project LLMs | `project` |
| 👥 **Contacts** | Who matters? | Humans + LLM assist | `contact` |
| 📨 **Comms** | What's in flight? | Multi-party | `message` |

The first five are the **memory core** — the recursive pattern
that scales from individual to org to project to agent. The last
three (Projects, Contacts, Comms) are the *life-and-work* layer
that the core memory composes around.

### 📥 Inbox — *"what's unprocessed?"*

The triage station. Notes here are **ephemeral by design** —
they get processed *out* into Journal, Wisdom, Projects, or
trash. The Inbox should approach zero on a regular cadence.

**Inbox is a per-sub-vault pattern, not a single global surface.**
Each accumulating sub-vault has its own Inbox:

- **Personal Inbox** — your own fleeting captures (this section).
- **Knowledge Inbox** — LLM-curated drafts pending human review
  before promotion to canonical Knowledge.
- **Project Inbox** — incoming items routed to a project (from
  Comms triage, external integrations, agent dispatch).
- **Comms Inbox** — the email/chat triage queue.

Each follows the same shape (queue + review SLA + triage actions
+ zero-inbox goal) but with sub-vault-specific actions. What
follows describes the Personal Inbox; other Inboxes are mentioned
in their respective sub-vault sections.

**The temporal contract** — between you and yesterday-you / future-you.
You must store every fleeting capture in a single trusted system,
and every day you must *read* yesterday's captures. Not necessarily
do them — just face them. (Borrowed from Tris's [FLAP system](https://www.youtube.com/@NoBoilerplate)
which borrowed from David Allen's GTD and Bob Doto's *A System for
Writing*.)

**Daily review is the system's entry point.** Opening Task lands
you in Inbox. Each fleeting note gets one of five triage actions:

1. **Do now** — task that takes <2 minutes. Do it, delete the note.
2. **Atomic-ize** — promote to Wisdom: rewrite in your own words,
   keep the original capture as a footnote (paper trail).
3. **Journal** — observation worth keeping but not synthesized.
   Drop into today's Journal entry with timestamp.
4. **File to Project** — belongs to ongoing work. Cut/paste into
   the project's notes.
5. **Snooze** — can't process today. Use spaced-repetition-style
   backlog management ("later / soon / now") so the note resurfaces
   at the right time without piling up.

Snooze is the key to scale: a backlog of thousands becomes a
processable stream of ~10/day. Spaced-repetition for fleeting
notes (re-purposed from study tools) is what makes the temporal
contract actually upholdable.

**Capture bridges** — Inbox accepts captures from anywhere:
in-app, mobile, Readwise (book/article highlights), Telegram /
SMS / email-to-inbox, REST API for scripts and external tools.
The home of capture is wherever you happen to be.

### 📓 Journal — *"what happened?"*

Stream-of-observation. Daily notes, meeting logs, agent run
transcripts, dream journals, decision logs, weekly retrospectives.
Time-ordered, append-style, kept indefinitely.

**Both humans and LLM teams keep journals.** Your daily reflection
sits next to the Ops agent's standup summary sits next to the
Knowledge-curator agent's "what I added today" log. The journal is
where the *operating log* of the org lives — what happened, who
did what, what got decided.

Journal entries can reference **Knowledge** (cite the facts
mentioned), **Wisdom** (cite the principles applied), **Projects**
(what work was advanced), and **Contacts** (who you talked to).
Journal cannot be referenced by Knowledge — that's the
directionality rule. Your raw observations stay personal even when
they cite published facts.

### 🧠 Wisdom — *"what do I understand?"*

Your atomic notes. Your synthesis. Things written *in your own
words* after you've digested them — the actual learning, not the
raw material.

This is the Zettelkasten layer, modeled directly after Bob Doto's
*A System for Writing*. Each atomic note is:

- **Loosely coupled** — readable on its own without chasing 12 links
- **Highly cohesive** — focused on one idea
- **Owned by you** — your phrasing, your understanding, not a copy-paste
- **Cross-linked** — see-also references to related atomic notes
- **Tree-positioned** — one `up:` frontmatter property gives it a
  single canonical parent, building a navigable tree

The tree-view (one parent per note) solves the "graph view
doesn't scale past 30 nodes" problem. By cycle 3 you have
hundreds of atomic notes; the up-link tree keeps them browsable
like a card-box.

**Wisdom can cite Knowledge** (your understanding rests on
facts), and **Wisdom can cite Wisdom** (ideas connect). Wisdom
cannot be cited by Knowledge — the encyclopedia stays
depersonalized.

File names are *identity*, not IDs — human-readable summaries
that jog your memory without opening the file. `Writers block is
caused by reader block.md` beats `2026-03-15-1342.md`.

### 📚 Knowledge — *"what is true?"*

**LLM-curated external knowledge.** Facts, references, definitions,
how-tos — the encyclopedia. Built as an
[LLM-Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f):
LLM agents read source documents (papers, books, web content,
internal docs) and incrementally maintain an interconnected
encyclopedia with citations and provenance. Same on-disk shape
Obsidian uses (Markdown + YAML frontmatter + `[[wikilinks]]`), so
the Knowledge vault works as a standalone Obsidian vault — full
[obsidian-compat](crates/obsidian-compat/) layer (parser, Bases
evaluator, CLI parity).

**Knowledge is a graph sink.** It can only link to other
Knowledge — no outbound links to Wisdom, Journal, or anything
personal. This isolation is what makes Knowledge **publishable in
isolation** (push to Quartz, share with collaborators, hand to a
new team member) without leaking personal context. Backlinks
panels still show incoming references from elsewhere — those are
queries, not authored edges.

Humans review LLM-curated entries before they're marked stable.
LLM-authored content carries provenance: `author: <llm-id>`,
`reviewed_by: <user>`, `prompt_hash: <…>`. You can always trace a
Knowledge entry back to the source documents and the agent that
synthesized it.

**Knowledge Inbox** — LLM ingestion agents drop drafts into the
Knowledge sub-vault's own Inbox (`knowledge/_inbox/`) tagged
`pending_review`. A human reviewer triages: promote to canonical
Knowledge (move out of `_inbox/`, drop the tag), request revision
(comment + bounce back to the agent), or reject (delete). This
mirrors the personal-Inbox temporal contract but at the
sub-vault level — same discipline of "anything captured will be
returned to."

### 🔒 Records — *"what must stay private?"*

The locked private vault. Passports, IDs, driver's licenses,
medical history, insurance, financial account numbers, legal
documents (wills, contracts, leases), tax returns, vaccination
certs, warranties.

**Records is graph-isolated by design.** It has no outbound
references and is referenced by nothing. **No LLM can ever read
it.** No exception, no override, no escape hatch — the
information-policy layer hard-blocks it.

Records carry **expiry metadata** — passport renewal date, license
renewal, warranty end. The Calendar lens surfaces upcoming
expirations as natural reminders. Stored with strong encryption
at rest; on-disk format is plain markdown so you can decrypt and
read with any tool if Task ever disappears.

### 📁 Projects — *"what am I building / doing?"*

Where Task shines. One pillar holds every project — personal,
work, side, archived — filterable by the organization switcher
("Personal", "FastTrackStudio", "Client X", …). Each project owns:

- Its own task list (with kanban, gantt, agent dispatch)
- Its own notes / spec docs / meeting logs
- Its own attachments
- Its own time + invoice records
- Its own subset of contacts (collaborators, clients, vendors)
- Its own **custom workflows** — status sets, kanban columns,
  automation, agent dispatch rules — defined per-project (or
  inherited from the org template).

**Crucially, a project's data lives where its actual artifacts
live** — alongside the code repo, the design files, the
spreadsheets. Task isn't a centralized knowledge silo you sync
into; it's the *editing surface for the data that's already where
your work lives*. Move a project, it's a folder move. Archive it,
it's a folder archive. Share it, push the folder to a collaborator.

### 👥 Contacts — *"who matters?"*

A CalDAV-synced relationship manager. Standard address-book fields
plus the interaction graph: which projects this person touched,
which meetings you've had, what they said, what you owe them,
upcoming gift / compliment / check-in reminders. Designed to help
you be *intentionally* attentive — the application equivalent of
keeping notes on people you care about.

Contacts link bidirectionally with Projects (collaborators) and
Inbox (mentions, reflections about people).

### 📨 Comms — *"what's in flight, and where does it belong?"*

The communications surface: email, chat, project threads, DMs.
Same triage shape as Inbox — incoming messages get processed and
either linked to a project, archived, snoozed, or deleted — but
the data lives differently from the other pillars because
communications are inherently **multi-party**.

**Why this surface looks different from the others:**

- **Not markdown files.** Messages carry too much structure
  (sender, recipient, thread parent, timestamps, read state,
  delivery state, attachments, encryption envelopes), too much
  volume (100k+ message archives), and too many simultaneous
  participants for plain files to handle. Storage is structured
  tables on the server (SeaORM / postgres in prod), with vox
  streaming to clients and a local read-through cache for offline
  access.
- **Server-authoritative.** Email comes from external systems
  (IMAP/SMTP). Chat needs real-time push with multi-party
  participation. Audit logs need legal-grade immutability. These
  break the local-first model — and that's correct, because the
  data belongs partly to the *other* people in the conversation.
- **Federated, not platformed.** The reconciliation with the
  guiding constraint: the server is *infrastructure*. You can
  self-host it. Data is exportable in open formats (mbox for
  email, Matrix/MLS for chat). The client always has a local
  replica of your view. No vendor lock-in — same model as running
  your own SMTP server.

**Capabilities:**

- **Email client** — IMAP/SMTP per-org accounts, unified inbox
  across accounts, project-link rules (auto-tag incoming mail by
  sender / subject / project membership), compose / reply /
  forward. Attachments route through the existing attachments
  feature.
- **Chat / threads** — every project gets a `#general` thread by
  default; add more as the project grows. Cross-project DMs for
  collaborator conversations that don't fit one project. Real-time
  via the existing vox WebSocket relay. Threading, reactions, read
  receipts, presence.
- **External bridges** — adapters for Matrix (federated), and
  optionally Slack / Discord / Teams (work accounts) so messages
  in those systems can be triaged into the same surface.
- **Search at scale** — full-text + recency-weighted ranking
  (tantivy in v1, embeddings for semantic search later).

**Triage flow** — the same temporal contract as Inbox, applied to
in-flight communications:

1. Incoming mail / chat lands in Comms with no project assignment.
2. The triage view surfaces overdue items honestly (no badges, no
   shame — just "these need a decision").
3. Triage actions: link to existing project, create new project,
   archive, delete, snooze.
4. Once linked, the message appears in that project's
   conversation panel — alongside its tasks, notes, and time
   entries. The full conversation history is one query.

**Permissions + audit:**

- Per-project ACL: who can read this project's conversation log.
- Per-thread ACL: implicit from the original to/cc + explicit
  grants.
- Every access is recorded in an append-only audit log. "Who read
  what, when" survives any future dispute.
- Linking a message to a project is itself a logged metadata
  event — visible to anyone with project access.

Comms threads through every other sub-vault: Contacts (the people
in the conversation), Projects (where the conversation belongs),
Journal (insights captured mid-conversation), Knowledge (citations
to canonical references), Goals (which goal does this thread
serve?).

### Reference policy — directional rules between sub-vaults

Sub-vaults are linkable but governed by **type-directional rules**
enforced at link-write time, at LLM-context-build time, and at
publication time. The rule:

```
Knowledge  →  Knowledge only
              (publishable in isolation; no outbound to personal)

Wisdom     →  Knowledge ✓   Wisdom ✓   Journal ✗   Records ✗
              (synthesis cites facts; not raw observations or secrets)

Journal    →  Knowledge ✓   Wisdom ✓   Journal ✓   Projects ✓   Contacts ✓
              (observations reference everything except Records)

Inbox      →  Anywhere — ephemeral; will be processed out anyway

Records    →  Nothing.  Nothing references it.
              (graph-isolated by design)

Projects   →  Anywhere except Records (unless explicitly granted)

Contacts   →  Knowledge ✓   Projects ✓   Comms ✓   (people anchors)

Comms      →  References anything by ID;
              inbound references to Comms governed by participant ACLs.
```

**Backlinks are queries, not authored edges.** A Knowledge entry
can show "referenced by 3 Journal entries, 2 Wisdom notes, 1
Project doc" without violating the rule — the entry itself
authored no outbound links. The reference-policy applies to
*links written into the source*, not to *queries computed at view
time*.

This is what keeps the Knowledge sub-vault **publishable**:
push it to Quartz, share with a new team, hand to a contractor
— the source files have no edges pointing at your personal
context. Only inbound queries, computed by whoever's viewing.

### Org-scale — sub-vault × owner

Each sub-vault is owned by either an **org** or a **person**.
Switching orgs (`Personal` / `FastTrackStudio` / `Client X`)
changes which sub-vaults are visible. Personal sub-vaults remain
visible across all orgs — they're yours.

```
                Personal         Org: FastTrackStudio    Org: Client X
Knowledge       rare             ✓ shared world-facts    ✓ industry refs
Wisdom          ✓ atomic notes   ✓ team synthesis        ✓ deliverables
Journal         ✓ daily notes    ✓ standups + agent log  ✓ meeting logs
Inbox           ✓ always         —                       —
                                 (Inbox is per-person)
Records         ✓ passports      ✓ legal docs            ✓ signed contracts
Projects        ✓ side work      ✓ org work              ✓ client work
Contacts        ✓ personal       ✓ team + vendors        ✓ client roster
Comms           ✓ personal mail  ✓ org email + chat      ✓ client thread
```

The same five-part memory core (Knowledge / Wisdom / Journal /
Inbox / Records) is **fractal** — it applies recursively:

- **One person** has personal Knowledge / Wisdom / Journal /
  Inbox / Records.
- **One team / org** has shared org-scoped versions.
- **One project** can have project-scoped Knowledge / Wisdom /
  Journal (decisions, learnings, operating log specific to it).
- **One LLM agent** has its own Journal (its activity log) and
  scoped read/write access into specific sub-vaults.

Anywhere a coherent memory needs to exist, the same five-part
structure applies. The reference-policy rules stay consistent at
every scale.

### LLM teams as first-class members

LLMs aren't tools that read your notes — they're **members of
orgs** with identities, scope, and authorship. They show up in
the org's members list. They keep journals. They author content
with provenance.

```yaml
endpoints:
  knowledge-curator-claude:
    role: "Maintains the Knowledge sub-vault"
    write_access:  [knowledge_inbox]
    read_access:   [knowledge, journal_org_public, comms_org_public]
    cannot_read:   [wisdom_personal, records, journal_personal,
                    inbox_personal]

  ops-journal-agent:
    role: "Standup summarization + agent run logs"
    write_access:  [journal_org]
    read_access:   [journal_org, projects, comms_org]

  personal-assistant-local-ollama:
    role: "Personal help — local model only"
    write_access:  [inbox_personal, journal_personal]
    read_access:   [knowledge, wisdom_personal, journal_personal,
                    inbox_personal]
    runs_locally:  true
    never_reaches: [records, comms]
```

**LLM-authored content carries provenance** — every note an LLM
writes records `author: <llm-id>`, `prompt_hash: <…>`, `model:
<name>`, `at: <timestamp>`. Human-reviewed entries also carry
`reviewed_by: <user>` + `reviewed_at: <…>`.

**LLMs have their own Inbox-and-temporal-contract too.** The
Knowledge-curator agent's drafts land in the Knowledge Inbox
awaiting human review. The Ops-Journal agent's daily summary
must be written by a SLA — if it skips three days, the
operator notices.

**Default deny + explicit grant.** A new LLM endpoint added to
the org sees nothing until classes are explicitly added. There's
no "give it everything" preset. Records is **hardcoded
unreachable** — no override flag exists.

This is what makes "LLM-assisted life management" responsible
rather than reckless. The architecture *names* LLMs as actors
that need scope, audit, and review — not invisible context
consumers.

### 🎯 Goals — *"am I building the life I said I wanted?"*

Goals aren't a separate pillar — they're **Projects with horizon
metadata and a charter**. The goal-as-project pattern means
everything you've already built (tasks, notes, financial tracking,
contacts, attachments) composes for free; the goal layer just adds
the spine that connects today's work to your future self.

Each goal-project carries:

- **Horizon** — `today / week / cycle / quarter / year / 5-year /
  10-year / life`. (Cycle is a 28-day / 4-week block; see *Time
  architecture* below.) A project can sit at any level; long-horizon
  goals contain shorter-horizon sub-projects.
- **Charter** — one of the project's notes captures *why* this
  matters, what success looks like, the cost (financial / time /
  opportunity), and the contingency ("if not by date X, then Y").
  The charter is what makes the goal survive setbacks.
- **Financial target** — optional `target_amount` + `target_date`
  pulled from Operations.Finance. The system computes the monthly
  rate needed and shows progress against it.
- **Sub-projects** — a 5-year goal decomposes into a tree of
  shorter projects. "Buy a house" branches into "build credit,"
  "save down payment," "research neighborhoods," each with their
  own milestones.
- **Supporting habits** — habits linked to a goal answer the
  question they're for. Skipping a habit prompts an honest review:
  *is this still serving Goal X?* — making the trade-off explicit
  rather than letting it drift.

Examples:

> **Buy a car** *(1-year, $30k target)*
> Charter: why this car, why now, what it replaces.
> Sub-projects: research models, save monthly, sell current vehicle.
> Tasks: visit dealerships, get preapproval, test drive list.

> **Buy a house** *(5-year, $100k down payment)*
> Charter: target neighborhood, family timeline, mortgage tolerance.
> Sub-projects: build credit (1yr), save down payment (5yr),
> research neighborhoods (2yr), search → offer → close (year 5).
> Habits: monthly savings rate, weekly listings review.
> Contacts: spouse, realtor, loan officer.

### ⚙️ Operations — *per-organization business utilities*

Separate from the main nav because the concerns are different.
Where the 4 surfaces are about *thinking and doing*, Operations is
about *running the business*:

- **Time** — tracking, weekly/monthly/yearly reports, billable vs.
  non-billable, per-project rollups.
- **Invoicing** — generate invoices from time entries + line items,
  email PDFs, track paid/outstanding.
- **Finance** — income/expense ledger, category breakdowns,
  per-project P&L, tax-year snapshots.
- **Inventory** — locations + physical things they hold. Studios,
  warehouses, home offices, storage units; the gear, instruments,
  furniture, supplies inside each; restock triggers; assignment to
  active projects ("this mic is in Studio B for the next session").

These live in an Operations panel reachable from the org switcher,
not from the main nav — they're tools, not surfaces.

### 🔍 Lenses — *cross-cutting views over the pillars*

Lenses are **perspectives, not silos**. Each lens aggregates data
that already lives in the pillars and presents it through a domain-
specific UI. You can add new lenses without adding pillars or
duplicating data.

**Built-in lenses:**

- **📅 Calendar** — time-axis view of anything dated: task
  due/scheduled, project milestones, time entries, contact
  birthdays + follow-ups, wiki entries with `date:` frontmatter.
  CalDAV bidirectional sync interoperates with your existing
  calendar app.
- **🗺 Map** — location-axis view: inventory locations (studios,
  warehouses), project venues, contact addresses, meeting points.
- **🕸 Graph** — link-axis view: the wiki internal graph, the
  cross-pillar reference graph, backlinks panels per entity.
- **🔁 Habits** — recurrence-pattern view: which behaviors you
  committed to, what the last 30/90/365 days looked like, what
  goals they serve. **No streaks, no gamification** — see the
  guiding constraint below. The view surfaces honest information
  (12 of 30 days, last gap 4 days), and skipped habits route to
  Inbox for review.
- **💪 Training** — workout aggregation: PR graphs, volume per
  muscle group, program adherence, deload signals. Built from
  tasks tagged with `workout:` structured data.
- **🍳 Meals** — meal-planning view: week grid, prep-day visibility,
  shopping-list generator (`project meal plan` minus
  `pantry inventory` = grocery list), pantry-aware recipe
  suggestions, expiry prompts.
- **🎯 Goals** — horizon pyramid (life → 10yr → 5yr → year →
  quarter → month → week → today). Up-link from any task ("what
  goal does this serve?") and down-link from any goal ("what am I
  doing this week toward this?"). Drift detection surfaces stale
  goals to Inbox for re-charter or drop.
- **💰 Finance dashboard** — category trends, per-project P&L,
  goal-savings progress, cash-flow projection.

**Custom lenses:** any user can define a new lens as a *query +
layout*. "Reading log," "Journaling streaks," "Gardening,"
"Apartment hunt" — none of these need new code or new pillars.
A lens is a saved cross-pillar query rendered through a chosen
visualization (list, grid, calendar, map, graph, kanban, gantt).

This is what makes the architecture scale: **new life domains
compose** the existing pillars through new lenses. They don't
demand new silos.

### Why this shape

Most knowledge tools either *centralize everything* (Obsidian =
one vault, Notion = one workspace) or *scatter into silos* (loose
folders, separate apps for tasks vs. notes vs. contacts). Both
modes have failure cases:

- **Centralized**: every piece of knowledge tangles with every
  other. The encyclopedia becomes unsharable because your fleeting
  notes leaked into it. Migration is a database operation.
- **Siloed**: context-switching between "where I work" and "where
  I write about my work." Forgetting where you put something.

Task's bet is **typed sub-vaults with directional reference
policies and project-colocated data**. Knowledge curated by LLMs,
Wisdom synthesized by humans, Journal capturing what happened,
Inbox triaging what's unprocessed, Records guarding what must
stay private. Linking is governed: Knowledge can't reach into
your personal stream, Records can't be touched by any LLM. The
maturation pipeline (Inbox → Journal/Wisdom → Knowledge) becomes
the natural shape of how information ripens.

The result: an encyclopedia you can publish, a personal
synthesis layer that stays yours, an operating log shared with
your team and your agents, and a private vault for the things
that need to stay private. Same five-part memory core for one
person, one team, one project, or one agent — fractal at every
scale.

### Time architecture — cyclic planning

Task plans time in **cycles** rather than calendar months. The
default time model:

```
Year
├── Q1  (13 weeks)
│   ├── Cycle 1   (4 weeks = 28 days)
│   ├── Cycle 2   (4 weeks)
│   ├── Cycle 3   (4 weeks)
│   └── Reset Week  ← end of quarter
├── Q2  (same shape)
├── Q3  (same shape)
└── Q4  (same shape)
```

Year totals: `4 × 13 × 7 = 364 days`. The 1–2 leftover days
accumulate over ~5 years until they cross a 7-day threshold,
giving that year a **Week Zero** at the boundary (a prep week
before Cycle 1 of the next year).

**Why cycles instead of months:**

Calendar months are awful for routines. They're different lengths
(28/29/30/31), they start on different days of the week, and the
number of each weekday per month drifts. Plan a "first Sunday of
the month" routine and the first Sunday floats anywhere from day
1 to day 7. Plan "midpoint reflection" on the 15th and it lands
on a different weekday every month.

Cycles fix this. Every cycle:
- Has exactly **28 days = 4 weeks**.
- Starts on the **same weekday** (user-configurable; default Monday).
- Has **4 of each weekday** — 4 Mondays, 4 Sundays, etc.
- Has weeks worth exactly **25% of the cycle** — planning math is
  trivial ("I want to be halfway by end of week 2").

This makes habits, training programs, meal-prep rotations,
journaling rhythms, and weekly reviews land predictably. The
midpoint of every cycle is always the same weekend. The end of
every cycle is always the same weekend. Routines build naturally.

**Reset weeks are structurally separate from cycles.** A reset
week is not "the fifth week of a cycle." It sits at the end of
each quarter as a deliberate gap — refresh spaces, review goals,
consider what worked, prep the next quarter. The quarterly review
*cadence* is built into the calendar itself; the system doesn't
need to remind you because the week IS the reminder.

**Anchor rules:**

- **Year start** — Week 1 of Cycle 1 of Q1 is the first 7-day
  week containing **≥ 4 days** of the new year (ISO-week-style).
  Monday-start, 2026: starts 2025-12-29.
- **Cycle epoch** — configurable per-user (Monday-start default;
  Sunday-start common for some traditions).
- **Bonus / Week Zero years** — for Monday-start: 2026, 2032,
  2037. For Sunday-start: 2025, 2031, 2036. The bonus week is
  treated as Week Zero of the *following* year — a prep / soft-
  start week before Cycle 1 begins.

**Two coordinate systems, one source of truth:**

Every datetime in Task has two representations, computed from one
`DateTime<Utc>`:

```
Gregorian:   2026-03-15
Cyclic:      2026-Q1-C3-W2-D5     ("year, quarter, cycle, week, day")
```

External interop speaks Gregorian — CalDAV, IMAP timestamps,
invoices, tax periods, anything the world's running on. **In-app
primary display is cyclic.** The Calendar lens dual-renders both;
goal horizons (cycle / quarter / year) reference the cyclic
coordinate; reviews fire on cyclic boundaries.

**Where this shows up:**

- **Goals** — horizons are cycle-aware (today / week / cycle /
  quarter / year / 5y / 10y / life).
- **Habits lens** — heatmap defaults to 28-day grid (4 weeks × 7
  days). Cadence templates speak in cycles ("once per cycle",
  "week 1 of each cycle", "twice weekly").
- **Training lens** — a training block is naturally a cycle
  (3 build weeks + 1 deload — the original mesocycle).
- **Reviews** — automatic ritual prompts on cycle/quarter
  boundaries. End-of-cycle weekend, end-of-quarter reset week,
  year-end Week Zero.
- **Inbox temporal contract** — review SLAs measured in cycle-
  weeks rather than calendar weeks (consistent meaning regardless
  of which month a note landed in).

Credit: this 4-quarter / 3-cycle-plus-reset-week structure
follows the cyclic-planning system documented at
[youtube.com/watch?v=BiY2yUwTgQc](https://www.youtube.com/watch?v=BiY2yUwTgQc).
A user who prefers traditional Gregorian planning can flip the
default; the cyclic layer becomes a parallel coordinate system
they can ignore.

### Information policy — classification, redaction, audit

Sub-vault membership is the **primary classifier**. A note's
sub-vault implies its default sensitivity class:

| Sub-vault | Default class | LLM access shape |
|---|---|---|
| Knowledge | `public` | Any registered LLM, including cloud |
| Wisdom | `personal` | Local LLM only (or org-scoped LLMs with explicit grant) |
| Journal | `personal` (`org-shared` if org-scoped) | Org-scoped LLMs; cloud requires opt-in |
| Inbox | `personal` | Local LLM only — your raw stream |
| Records | `records` | **Never sent to any LLM.** Display only. |
| Projects | `project` | LLMs scoped to that project |
| Contacts | `personal` / `org-shared` | Org-scoped LLMs |
| Comms | governed by participant ACLs | Conversation-scoped LLMs only |

**Per-note overrides** for edge cases — when a Wisdom note
happens to contain sensitive data (a research note about your
therapy progress, say), the `sensitivity:` frontmatter field
upgrades it: `sensitivity: records`. Downgrade is similarly
explicit. Override never **expands** access beyond what the
sub-vault allows; it can only restrict further.

**LLM endpoint registry** — each LLM in the org declares which
sub-vaults / classes it's allowed to see (full example in the
LLM Teams section above). Endpoints with no explicit grant see
nothing. Records is hardcoded unreachable for every endpoint.

**Pre-flight redaction** — every context-building pipeline (chat
agent, summarization, semantic search, knowledge-curation
ingest) filters by the target LLM's allowed scope *before* the
prompt is built:

- Disallowed notes don't enter context at all.
- Citations to disallowed notes get redacted at the link
  (`[redacted: passport.md (records-class)]`) so the LLM knows
  *something exists* but not what.
- Records-class notes are never included by any pipeline — no
  override, no escape hatch, no exception.

**Audit log** — every prompt to every LLM endpoint records
which notes were in context, their classes, and the receiving
endpoint. Append-only. "What does Claude know about my Personal
org?" is a single query against the audit log.

**Records lens** — surfaces every records-class note across the
vault with the audit context visible: "passport.md (Records,
expires 2029-04, no LLM has ever seen it)." Per-record expiry
metadata routes to the Calendar lens as renewal reminders.

This is what makes "LLM-assisted life management" responsible
rather than reckless. The default isn't "send everything to the
smartest model"; the default is "the smallest model that respects
the data's class — and **never** Records."

### The guiding constraint

**Software meant to improve / augment your life, not consume your
life.**

Every design decision passes through this filter. Concretely:

- **Output beats input.** Features must produce visible life
  improvement (less forgotten work, better relationships, time
  reclaimed). Capture time that doesn't return value is dead
  weight. If a feature mostly grows the app's content without
  changing how the user lives, it's a smell.
- **In-and-out, not all-day.** Get information in fast, get
  answers out fast. Long sessions inside the app are a failure
  mode. Workflows route you back to the world.
- **Push to the world, not pull into the app.** Calendar entries
  sync out via CalDAV. Notes live as files on disk you can edit
  in any editor. Contacts federate. Task is a lens over your
  existing life-data, not a destination that owns it.
- **No engagement loops.** No streaks, no gamification, no
  daily-active-user hooks. The temporal contract is a real
  obligation surfaced honestly — "you've ignored these 12 notes
  for 3 weeks, here they are" — not a habit-trap dressed up as
  productivity.
- **Quiet by default.** Notifications only when they carry signal
  the user would want acted on (a contact's birthday tomorrow,
  a deadline approaching, a payment overdue). Never for
  attention-fishing.
- **Owns nothing, federates everything.** Your data is plain
  files in standard formats. Markdown, YAML frontmatter,
  iCalendar, vCard. You can stop using Task at any time and your
  data continues to work in every other tool that reads those
  formats.
- **Local-first is an ethic, with one honest exception.** No
  telemetry, no cloud dependency, no subscription lock-in. The
  sync relay you self-host or skip entirely. The exception is
  Comms: emails and chats involve other people's data, so the
  server is system-of-record rather than relay — but it's still
  *your* server (self-hostable), in open formats (mbox, Matrix),
  with a full local replica. Federated infrastructure, not
  platform lock-in.

The scope is *entire life management* — projects, knowledge,
relationships, communications, time, money, things, places. The
constraint is that all of it has to serve the life it's managing,
not become it.

## What the words mean here

- **Local-first.** The user's data is a folder of markdown files the
  user owns. The vault is the source of truth; indexes and databases
  are rebuildable from it. Delete the server and the data is intact.
- **Realtime.** Edits to a vault file propagate in milliseconds over a
  WebSocket. Open two tabs on the same note and watch them stay in
  lockstep — that path is backed by [Loro](https://loro.dev/) CRDTs
  (see `docs/architecture/vault-crdt-reconciliation.md`).
- **Collaborative / Multiplayer.** For collaborative *text*, no "save"
  button and no conflict dialogs: CRDTs merge concurrent edits
  deterministically and the write-behind loop projects the result back
  into the file.
- **Extensible.** Every domain is a slice under `features/task/`,
  exposing an `#[architect::rpc]` service trait. The common shape is
  `<slice>-proto` (wire types + service traits) plus a `<slice>`
  facade, with optional `-db` (sea-orm), `-ui` (Dioxus), and `-live`
  (filesystem backend) crates. External integrations — agent backends,
  forge sync, email backends — plug into trait-shaped seams without
  touching the core.

## UI rules

**All UI components must be compatible with the theming system.**
This is non-negotiable.

- Build on `architect-ui` primitives (`Button`, `Card`, `Sheet`, `Dialog`,
  `Combobox`, `Sidebar`, etc.). Avoid hand-rolled equivalents unless
  there's a specific reason architect-ui can't cover the case — and then
  fix it upstream in architect-ui rather than working around it.
- Use **theme tokens** for color: `bg-background`, `text-foreground`,
  `bg-card`, `border-border`, `bg-primary`, `text-muted-foreground`.
  Never hardcode `bg-slate-*` or hex colors. The token values come
  from the active preset; switching preset (or flipping dark mode)
  must change the whole app's appearance without component edits.
- **Dark mode is the default.** Components must look correct in both
  light and dark with no `dark:` overrides — the CSS variables flip
  values per mode and your component just consumes them.
- **Two-tier theming.** Each *organization* picks a preset (default,
  violet-bloom, supabase, t3-chat, neo-brutalism, etc.). Each
  *project* can optionally override its org's theme. This is wired
  via `architect_ui::ThemeProvider` at the App root and `ThemeScope` inside
  the project route. New theme-aware surfaces should respect both
  tiers — don't bypass the provider.
- **Dumb components.** Feature `*-ui` crates own no state: data in,
  events out via `EventHandler<T>`. Signals and vox clients live in
  the page (`crates/task/ui/src/pages/`). This keeps components
  portable across web/desktop/mobile and usable in ui-lab.

When a component you need doesn't exist in architect-ui, prefer:
1. Compose it from existing architect-ui primitives, or
2. Add it to architect-ui — it lives in-tree at `libs/architect-ui/architect-ui` as a
   path dep, so edits propagate on the next `cargo check`.

## Architecture in 30 seconds

```
features/task/<slice>/
  <slice>-proto/   entity types + #[architect::rpc] service traits;
                   architect emits Client / Dispatcher / descriptor
  <slice>/         facade + backend implementation
  <slice>-db/      sea-orm entities + migrations   (rare)
  <slice>-ui/      dumb Dioxus components          (rare)
  <slice>-live/    filesystem backend + watcher    (vault, wiki)

apps/task/server    task-server: axum + one architect LayerRouter per
                    org, served at /org/{slug}/vox
apps/task/{web,desktop,mobile}
                    Dioxus platform shells over crates/task/ui
apps/task/cli       package task-cli → the `task` binary

crates/task/ui      package `ui` — the Dioxus app shell: router,
                    pages/, theming, stores, vox session
```

The shape varies by slice: several are proto-only, and a few (`task`,
`project`, `goal`, `cycle`) declare their service trait inside the
facade crate rather than a separate proto. Read the neighbour before
copying it.

Storage is three-tiered: **markdown + YAML frontmatter in the vault**
for most entities (the source of truth), **sea-orm** for a minority
(agent-task queue, timer, threads, prefs, finance) plus architect's
auth / permissions / share tables, and **Loro CRDTs** only for
collaborative editing of vault markdown files.

Full detail: [ARCHITECTURE.md](ARCHITECTURE.md).

## Quick start

```bash
# Enter the dev shell (direnv loads it automatically on cd from the
# repo root). Manual: nix develop <repo-root>

# From apps/task/ :

# Terminal 1 — the server
just server                   # listens on :9090

# Terminal 2 — the Dioxus dev server
just web                      # listens on :8765

# Or both in one process:
just dev
```

There is no separate migrate/seed step: the server resolves its data
root from `$TASK_DATA_ROOT` (default `$HOME/.task`), creates
`orgs/<slug>/` on demand, and runs each service's migrations at boot.
Point `TASK_DATA_ROOT` at a throwaway directory for a clean slate.
See [`.env.example`](.env.example) for the full env surface.

## Common recipes

```bash
just check         # cargo check --workspace  (the WHOLE monorepo)
just build         # cargo build --workspace
just test          # cargo test --workspace
just fmt           # cargo fmt --all
just clippy        # cargo clippy --workspace --all-targets -- -D warnings
just ci            # fmt --check + clippy + nextest run
```

`--workspace` means all ~160 monorepo members, not just Task — use
`-p <crate>` when you only care about a Task crate.

## Adding a slice

There is no scaffolder (`cargo xtask` only does TS codegen). Copy the
nearest existing slice:

1. Pick a neighbour whose shape matches what you're building.
2. Define entities + the `#[architect::rpc]` trait in
   `<slice>-proto/src/` (or in the facade, if you're following a slice
   that does it that way).
3. Implement the backend in the facade crate — over `vault::Vault` for
   markdown-backed entities, or sea-orm if the data genuinely isn't
   file-shaped.
4. Register the dispatcher in `org_layer_router` in
   `apps/task/server/src/lib.rs`, and add its schema stamp alongside.
5. Build components with `architect_ui::prelude::*` and theme tokens; wire
   the page in `crates/task/ui/src/pages/` and the route in
   `crates/task/ui/src/routes.rs`.

Remember: touching a proto changes vox method ids — rebuild and
restart task-server before trusting live behavior.

## Status

Active development. Data lives under `$TASK_DATA_ROOT` (default
`$HOME/.task`) and persists across restarts.

## Credits

The product architecture builds on ideas from several sources
worth following directly:

- **Andrej Karpathy** — [LLM Wiki gist](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f).
  The pattern of LLM-curated, incrementally-maintained knowledge
  bases. Task's Knowledge sub-vault is a direct implementation.
- **Tris (No Boilerplate)** — [the FLAP system](https://www.youtube.com/@NoBoilerplate)
  + [namarie.com](https://namarie.com). Five years of refined
  Zettelkasten-meets-GTD practice in Obsidian. Inbox triage,
  the temporal contract, spaced-repetition backlog management,
  the up-link tree structure, file-names-as-identity — all
  borrowed and extended.
- **Bob Doto** — *A System for Writing*. Definitive book on
  modern Zettelkasten / atomic notes. Task's Wisdom sub-vault is
  the Zettelkasten layer.
- **David Allen** — *Getting Things Done* (2001). The
  single-trusted-system principle, the 2-minute task rule, the
  capture/process/organize/review/engage workflow shape.
- **[Cyclic planning system](https://www.youtube.com/watch?v=BiY2yUwTgQc)**
  — the 4-quarter × (3-cycle + reset-week) time architecture.
- **[nashsu/llm_wiki](https://github.com/nashsu/llm_wiki)** —
  Tauri+React implementation of Karpathy's LLM-wiki pattern.
  Reference for the LLM-curation pipeline we want to build for
  the Knowledge sub-vault.
- **[Obsidian](https://obsidian.md)** — Markdown + YAML
  frontmatter + `[[wikilinks]]` as the on-disk format Task adopts
  for everything text-shaped. The
  [obsidian-compat](crates/obsidian-compat/) layer keeps Task's
  knowledge vaults openable in Obsidian if you ever want to
  leave.

## License

Dual-licensed under either of

- Apache License, Version 2.0 (`LICENSE-APACHE`)
- MIT license (`LICENSE-MIT`)

at your option.
