# Project Spec

What a project is, how projects nest, what a project can *do*, and what it
produces. Answers to charters
[#13 The Ideal Project Organization](https://github.com/FastTrackStudios/task/issues/13)
and [#18 Project Domain Hierarchy](https://github.com/FastTrackStudios/task/issues/18);
the storage layer beneath it is [`../../files/spec/files.md`](../../files/spec/files.md).

Four ideas, deliberately not collapsed into one `project_type`:

| | Question | Cardinality |
|---|---|---|
| **Capability** | what work happens here | a set — `music-production`, `video-production` |
| **Form** | what this thing *is* | one — `song`, `album`, `concert`, … |
| **Part** | a unit of the work | many, and **not** a project unless promoted |
| **Nesting** | what it belongs to | one parent |

Two real projects the model has to fit:

- **A concert film.** Capabilities `video-production` + `music-production`, form
  `concert`. Its songs were chopped out of one piano recording — they are
  **parts**, not projects, and creating fifteen projects for them would be
  ceremony with no work behind it.
- **A worship album.** Capability `music-production`, form `album`. Each of its
  fifteen songs carries its own Pro Tools and Reaper sessions and thousands of
  files — those songs **are** promoted to subprojects, and the model must let
  them be without the album being built differently.

Same grammar, opposite answers. Which one a song gets is a judgement about the
work, made per song, and reversible.

---

## Structure

### A subproject is a project

t[project.nesting.uniform]
There is one project entity and it nests without limit. A subproject differs
from its parent only in having one, and anything true of a project is true of a
subproject — capabilities, deliverables, files, tasks, time. Nothing is a
project only by virtue of its depth, and no level is special-cased.

---

### A project is declared in frontmatter

t[project.identity.declaration]
A project is a markdown document whose frontmatter declares `type: project`.
That document carries the project's stable id, capabilities, form and parent,
and is an ordinary note — greppable, and editable in any editor — so nothing
outside the file is needed to interpret it. A directory holding no such document
is not a project: it is unclassified content, which stays browsable and
adoptable.

---

### Identity survives renaming and moving

t[project.identity.stable]
A project keeps its identity across rename, move, re-parenting, promotion and
migration between orgs and servers, so links, tasks, time and files stay
attached. The id lives in that frontmatter rather than in an external index, so
a project carried to another machine by `cp -r` arrives intact, and a project
renamed by someone in Finder is still the same project.

---

### An existing directory becomes a project in place

t[project.identity.adoption]
An existing tree is adopted as a project without moving, copying or renaming a
byte, and stays usable by the applications already writing to it during and
after adoption. Adoption is incremental and interruptible: a partially adopted
tree is browsable, and resuming does not restart. A directory with no marker is
adoptable — absence of metadata means unclassified, never invisible.

---

### Nesting is declared, not inferred from the filesystem

t[project.nesting.explicit]
Parentage is a declared link, not a consequence of directory containment. A
project's files usually live under its parent's directory and need not.
Hardcoded directory names — `Projects/`, `Albums/` — express no hierarchy and
are not consulted.

---

## Parts

### A part is a unit of work, not a project

t[project.part.unit]
A project's work divides into named parts — a song, a scene, an episode. A part
is addressable, carries deliverables and components, and costs nothing: no
directory, no marker, no capabilities of its own. Work that is merely a segment
of the project's output stays a part, and dividing an album into songs never
requires creating anything project-shaped.

A part is independent of whether its content has been materialised. A song
rendered to its own audio file is still a part; so is one that exists only as a
region of a longer recording (see `files.index.regions`). Having real files
neither makes a part a project nor stops it becoming one.

---

### A part can be promoted, and demoted

t[project.part.promotion]
A part is promotable to a subproject when it earns one — its own files, tasks,
sessions, capabilities or collaborators — and a subproject is demotable back to
a part. Promotion preserves identity: links, deliverables, setlist references
and time already attached to the part continue to resolve, and nothing that
referenced it needs to know which side of the line it now sits on.

Identity is preserved by **reuse, not by mapping**: the subproject a part
becomes carries the part's own id, and demotion returns that same id to the
parent's parts. There is no table relating a part id to a project id, because
such a table is a thing that can be missing, stale, or unavailable on a machine
holding only half the project — and every reference the rule promises to keep
resolving would go through it.

A parent's part list is a **roster of its pieces, not a list of the ones that
are not projects**. Promotion adds a page; it does not edit the roster, and
demotion removes the page and leaves the roster alone. So an album lists ten
songs before and after three of them are promoted, in the same order, and
promotion is reversible without the parent being touched at all.

That leaves "is this piece a project?" with exactly one answer, from exactly
one place: whether a page declares it. A roster entry is not a competing claim
— it says the album has a fourth track, which stays true either way.

---

### A project's pieces are one list, however they are stored

t[project.part.listing]
Every piece of a project's work is enumerable in one call, in the project's own
order, whether each piece is a part or a promoted subproject. A caller asking
what an album consists of gets ten songs, and learns which of them have pages
only if it asks.

This is what makes the promotion rule true rather than aspirational. Without
it, "nothing that referenced it needs to know which side of the line it sits
on" holds only for callers that already had an id: everything that starts from
the project — a setlist being assembled, a deliverable scoped per song, a
person reading a track listing — would have to query two surfaces and merge
them, and would therefore need to know exactly the thing the rule says it
should not.

Order is the project's, and survives promotion — it is the roster's order, and
promotion does not touch the roster. An album's fourth track is its fourth
track before and after it grows a page.

---

### Demotion refuses what a part cannot hold

t[project.part.demotable]
A subproject is demotable when everything it carries fits in a part. Where it
does not — it has subprojects of its own, which a part cannot have — demotion
is refused, and says which. The alternative is a demotion that silently orphans
or deletes, and `project.part.promotion` is a promise about not losing things.

Content is not among the obstacles. A subproject holding files, tasks, time or
deliverables demotes with all of them still attached, because a part is
addressable and carries exactly those — that is `project.part.unit`, and it is
the reason demotion can be offered at all.

---

## Capabilities

### Capabilities are a set, not a type

t[project.capability.multiple]
A project declares zero or more capabilities describing the work it supports:
`music-production`, `video-production`. Holding both is normal and additive — a
concert that is recorded, mixed and cut holds two, and both sets of conventions
apply at once with no precedence between them.

---

### A capability determines conventions

t[project.capability.conventions]
Each capability declares the facets its work produces (see
`files.sync.selective`), the artifacts to ignore or absorb (see
`files.version.native`), its checkpoint cadence, its deliverable kinds, and the
UI surfaces the project offers. A project with no capability still stores,
syncs and versions; it simply offers no specialised surface.

---

### Capabilities change over a project's life

t[project.capability.mutable]
A capability is added or removed at any time, including on a project already
full of content. Adding one brings its conventions, facets and surfaces to work
that already exists. Removing one withdraws them without touching a byte: the
content it produced stays browsable and versioned, simply without a specialised
surface. Removal is never a deletion, and re-adding restores the surface over
the same content.

---

### Capability vocabulary is closed and small

t[project.capability.closed]
Capabilities are a fixed enum, extended only in code. The set must stay small
enough that a sync client, a placement policy and a UI can each reason about
every member. A need that does not fit an existing capability is a request for
a new one, never a free-form string.

Closed is a claim about what the system **writes**. A vault is hand-editable
and gets edited elsewhere, so a name outside the vocabulary is read as *no*
capability and reported as unrecognised — never guessed at, and never written
back. A project whose declaration nobody can interpret is a project with no
conventions, which is a legible state; inventing one for it is not.

---

## Form

### Form declares what a project contains

t[project.form.grammar]
A project has at most one form, and each form declares the parts it expects. An
`album` expects songs; a `concert` expects songs. The grammar describes parts,
which are promoted or not independently, so form says nothing about how many
subprojects exist. A project that does not match its form is valid and flagged,
never rejected.

---

### Form is a closed vocabulary, and optional

t[project.form.closed]
Form is a fixed enum extended only in code — `song`, `single`, `ep`, `lp`,
`album`, `concert`, `live-set` — so conventions and UI can be written against
every member. It is also optional: a project whose shape is not one we recognise
declares no form and is valid, nestable and complete without one. Unclassified
is a real state, not a catch-all member of the enum. A user-defined form
vocabulary is deliberately deferred until the shapes in real trees are known.

---

### Form declares optional components

t[project.form.components]
A form declares the components that may attach to it and to its parts. A song
optionally carries a chart and zero or more sessions; a session is a component,
not a project. A component is absent, present, or present several times — never
silently invented — and components survive a part's promotion unchanged.

---

## Lifecycle

### Two projects merge into one

t[project.lifecycle.merge]
Two projects created independently — different people, different orgs, different
servers, no common ancestor — merge into a single project. Capabilities union,
parts and deliverables combine, and content composes without moving bytes (see
`project.location.composed`). Where the two disagree — on form, title or the
identity of a part — a human chooses, and nothing is silently discarded. Merging
is the normal outcome of two halves of one job being started separately, not a
recovery from error.

---

### A merged project answers to both its former identities

t[project.lifecycle.merge-identity]
After a merge, references to either original continue to resolve: links, tasks,
time entries, and share links already in a client's hands. A former identity
resolves to the merged project rather than dangling, and the merge records what
it absorbed so the history stays legible to someone who only knew one half.

---

## Deliverables

### A deliverable is a first-class thing

t[project.deliverable.kind]
A deliverable is named, versioned output, distinct from the working files that
produced it. Each carries a medium (audio, video, image, document), a scope,
and an audience (internal, client, public). Deliverables are declared by the
project, not discovered by guessing which renders look final.

---

### Deliverable scope spans parts

t[project.deliverable.scope]
A deliverable's scope is the whole project, one per part, or an excerpt. A
concert therefore produces whole-project video, whole-project audio, per-part
audio, per-part video and a set of excerpts — five declarations, not
twenty-one files someone names individually. Per-part deliverables stay in step
as parts are added or removed, and are unaffected by whether a part is promoted.

---

### A declaration is not yet a file

t[project.deliverable.binding]
`project.deliverable.kind` says deliverables are declared rather than discovered
by guessing which renders look final. The consequence is that a declaration and
the content satisfying it are two things, and the content is attached
deliberately: nothing becomes the album master by being named `master.wav`.

So an item may be **declared and unbound**, and that is a legible state rather
than an error — it is a project saying what it owes before it owes it, which is
what a deliverable list is for at the start of a job. A client view shows such
an item as outstanding rather than hiding it, because "the per-song video is not
done yet" is the answer the client came for.

An excerpt is the case that makes this obvious. `project.deliverable.scope`
expands whole-project and per-part declarations on its own, and cannot expand an
excerpt: which seconds of which recording is a choice, so an excerpt exists
exactly when something has been bound to it.

---

### The client sees deliverables, not the tree

t[project.deliverable.client-view]
A client-facing view presents deliverables organised by scope and medium — the
whole performance, a specific song, a clip — and never the working tree that
produced them. Reaching any single deliverable takes one obvious path, and
audience determines visibility: nothing marked internal is reachable from a
client's view.

---

## Live performance

### Live-set projects feed setlists

t[project.setlist.source]
A project whose form is `live-set` exists to be performed: its songs carry the
tracks, charts and structure a performance consumes, and a setlist is assembled
by reference rather than by copying. A setlist references songs as parts,
promoted or not. Reordering or re-scoping a setlist changes no project, and a
song may appear in any number of setlists.

---

## Distribution

### One project, many locations

t[project.location.composed]
A project composes content from several storage locations at once, and this is
the normal case rather than a degraded one. A video company's server may hold
the footage while an audio company's server holds the sessions, and both are
the same project: one tree, one set of deliverables, one identity. No location
is privileged as the project's "real" home, and adding or removing one changes
where bytes live, never what the project is.

---

### One project, many servers and orgs

t[project.location.federated]
The locations composing a project may sit on different servers, owned by
different orgs, administered by different people. A contributor is a principal
who may be resident on another server, and their access derives from a grant on
the content rather than from being a member of whichever org happens to hold it.
Cross-org collaboration is expressed as grants, never by duplicating a project
per org or by nesting it under an org directory. The federation model itself is
charter [#22](https://github.com/FastTrackStudios/task/issues/22).

---

### An unreachable location hides content, not structure

t[project.location.degraded]
When a location or server composing a project cannot be reached, the project
still opens: its structure, parts, deliverable list and metadata remain
browsable, and the unreachable content is visibly absent rather than silently
missing. This is what the replicated catalogue is for — see
`files.catalogue.offline`. Work continues on everything still reachable, and
reconnection requires no re-adoption.

---

## Migration

### One definition of "project"

t[project.definition.single]
Exactly one definition of a project is in force across the vault, the Files
layer, the CLI and the UI. A directory-driven definition and a metadata-driven
definition must not coexist; where they disagree today, the metadata-driven one
wins and the directory list is deleted rather than kept in sync.

---

### The vault writes through the Files API

t[project.vault.write-path]
The production vault becomes a File Root and all writes to it go through the
Files API, markdown included. Its live tree stays ordinary files on disk —
greppable, `cp -r`-able, editable by any other tool — so migration changes the
write path and not the on-disk result.

**Met.** It was a migration rather than a feature, and what made it tractable
was that there is one choke point: every vault page in the system is written by
`vault_live::mutate` — `save_page`, `create_page`, `delete_page`, and the
`*_at` helpers the entity backends use for one-page writes — so the write path
is a *port* (`vault_live::PageSink`) bound per vault root. `FilesBackend::
adopt_vault` makes the org vault a File Root in place at server boot and binds
a sink for it; from then on a page save is a Files write — atomic in the tree,
a catalogue delta, a cadence hint — and a vault with no sink bound writes to
the filesystem as before. The on-disk result is unchanged: the same markdown at
the same path, which `tests/integration/tests/vault_root.rs` checks byte for
byte.

The "one choke point" claim had been optimistic. `project::write` was not the
last bypass: `task`, `goal`, `milestone` and `workstream` each wrote with a bare
`std::fs::write`, and every backend deleted and moved pages with `remove_file`
and `rename`. All of them now go through the port.

---

## Decided

**"Part" is the word.** It covers a song, a scene and an episode without being
a project. Considered and rejected: *piece* (concrete, but costs a rename
across every rule here), *item* (collides with inbox items, setlist items,
deliverable items, and greps badly), *work* (already means the whole project's
output in the layout reader's vocabulary, so it is ambiguous at the wrong
level). The word lands in the lane's verbs, the frontmatter key and every
error message, so it was settled before any of those existed.

**A form's expected parts are never created automatically.** `add_part` is
explicit. Adoption does not touch the project lane: reading a tree of fifteen
song folders is not consent to write fifteen entries into somebody's project
page, and a proposal surface can be added later without a migration because the
ids already exist. An `album` that expects songs and has none is a project
someone has not finished describing, not an error.

**The legacy `projectType` is read, not written.** Every project page predates
capabilities and carries a free-string type. It is interpreted on read where it
maps onto a capability, dropped on save once its meaning has been carried
across, and left untouched where nobody could interpret it — deleting a value
we could not read would destroy the only record of what its author meant.

## Open decisions

Recorded because they are not yet settled, and each blocks a rule above.

**How two sessions for one song relate.** `project.form.components` lets a song
carry both a Pro Tools and a Reaper session, which the real tree does. Whether
that pair needs relating to each other — same work, different tool — rather
than merely co-existing is unanswered.
