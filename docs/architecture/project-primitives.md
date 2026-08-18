# Building `project.*`

**Status: `project.*` is built, except the write path.** 23 of its 24
rules are covered and verified; `project.vault.write-path` is a
migration rather than a feature and the spec now records what it needs. The three
blocking decisions were answered — the word is "part", capabilities read
both fields and write one, and nothing is created automatically. See
`features/project/spec/project.md` § Decided for the reasoning, and
`tests/integration/tests/parts.rs` for what is asserted.

Each slice added rules the spec was missing, which is the pattern worth
noting: promotion needed `project.part.listing` and
`project.part.demotable`, and corrected how a parent tracks its pieces
(a roster, not a list of the unpromoted ones); deliverables needed
`project.deliverable.binding`. The spec is 119 rules now, up from 116.

Merge landed the way that predicted: the absorbed project becomes an
alias rather than being deleted, and `same_as` — parsed since before any
of this and read by nothing — is what it becomes an alias *through*.

**What remains is `project.vault.write-path`**, and it is a migration.
The useful finding is that it is one choke point rather than eighty call
sites: every vault page in the system is written by
`vault_live::mutate::write_atomic`. See the rule in the spec.

Two things found on the way, neither fixed here:

- **`project.vault.write-path` is not met**, and is the last one. Project
  pages no longer bypass the shared write path — they were the only
  caller doing a bare `std::fs::write`, which also made them the one
  vault entity a power cut could tear in half — so the migration is now
  one function plus a port.
- ~~**`ProjectInfo` has no `Default`.**~~ Fixed while adding
  deliverables: the impl is hand-written (three fields have defaults
  that are not their type's) and the seven literals now use it.

The original proposal follows.

---

A proposal, not a decision. `features/project/spec/project.md` describes
parts, promotion, capabilities, form, deliverables and merge; the code
implements none of them. This says what the smallest useful slice is,
what order the rest should come in, and which questions have to be
answered before anything is built.

## Where things actually stand

The project lane is CRUD over a markdown page:

```
list · get · get_by_path · create · update · rename · delete · events
```

`ProjectInfo` carries `id`, `parent_id`, `same_as`, `project_type`,
plus status/priority/lead/dates/billing. The parser already does three
things worth keeping:

- **`id` is stable** — from frontmatter, or a deterministic v5 fallback
  over the path when absent. `project.identity.stable` is most of the
  way there already.
- **`parent_id` exists** — declared nesting, not inferred from the
  filesystem, which is what `project.nesting.explicit` asks for.
- **`same_as` is parsed** — but nothing reads it. It is a field, not a
  feature, and `project.lifecycle.merge-identity` is the rule it wants
  to become.

Of 24 `project.*` rules, one has an implementation marker
(`identity.declaration`). Everything below is unbuilt.

## What blocks what

Sixteen of the scenario's twenty-five stages need `project.*`. They do
not need all of it, and the dependency shape is lopsided:

| build this | unblocks |
|---|---|
| **parts** | `piano.parts`, and half of `album.deliver` |
| **capabilities** (closed set) | `album.promote`, `piano.capability-churn`, and `files.facet.tool-layout`'s conventions |
| **promotion/demotion** | `album.promote`, `piano.promote-demote` |
| **deliverables** | `album.deliver`, `album.client-link`, `album.setlist` |
| **merge** | `piano.merge`, `piano.merge-identity` |

Parts and capabilities are the base. Everything else stands on them, and
merge stands on all of it — which is also the order of increasing risk,
so it is a usable build order as written.

## The first slice: parts

**Why first.** A part costs nothing by definition — "no directory, no
marker, no capabilities of its own" — so it is the one primitive that
adds no storage, no migration and no new failure modes. It is also the
one the example tree is already full of: `Example Album` has three
tracks and no `project.md`, which is the exact case
`project.part.unit` describes.

**Shape.** Parts are a list in the project's own frontmatter, because
the spec says nothing outside that file is needed to interpret it:

```yaml
---
type: project
id: 018f...
title: Crescendum
parts:
  - id: 018f...
    name: Overture
  - id: 018f...
    name: Daybreak
---
```

Each part gets a stable id from the moment it is named — that is what
makes `project.part.promotion`'s "links, deliverables and time continue
to resolve" achievable later, and retrofitting ids onto parts that
already have references is the migration this avoids.

**What it needs from the lane:** `parts(project)`, `add_part`,
`rename_part`, `remove_part`. Four verbs, no new store.

**What it proves:** `project.part.unit` and `scenario.piano.parts`, and
it makes the refusal in that stage testable — "creating seven projects
for seven regions of a single recording is refused as ceremony" needs
something to refuse *in favour of*.

## The second slice: capabilities

**The migration question this raises, and it is the real one.**
`project_type` is a free string today. `project.capability.closed`
requires a closed, small vocabulary, and `project.capability.multiple`
requires a *set* rather than one value. Those are the same field, so
this is a breaking change to every project page in every vault.

Three options, and I recommend the third:

1. **Replace `project_type` with `capabilities`.** Cleanest model,
   breaks every existing page, and there is no in-place migration for
   vaults we do not host.
2. **Keep both, `project_type` deprecated.** No break, and two fields
   meaning nearly the same thing forever — which is exactly what
   `project.definition.single` exists to prevent.
3. **Read both, write `capabilities`.** `project_type: video` parses as
   `capabilities: [video-production]`; anything unrecognised parses as
   no capability and is reported rather than guessed at. A page is
   migrated the next time it is saved, and a page nobody edits keeps
   working. The closed vocabulary is enforced on the write path only.

Option 3 makes the closed vocabulary true of everything the system
*writes* without pretending it is true of everything it reads, which is
the honest version and the one that survives a vault edited in Obsidian.

**Vocabulary to start with:** `music-production`, `video-production`.
Those are the two the example tree and the scenario use. Adding a third
should require a reason and a conventions table, which is the point of
the vocabulary being closed.

## Then: promotion, deliverables, merge

**Promotion** is where parts and capabilities meet: a part becomes a
subproject with its own page, keeping its id. Demotion reverses it,
keeping the id again. The test is not "does it move" but "does
everything that referenced it still resolve", which is why the ids in
slice one matter.

**Deliverables** are a first-class kind with a scope that spans parts —
"per-song audio for all ten songs" as one declaration, not forty file
references. This is the slice the client-facing surfaces need, and
`album.client-link` is the stage that finally makes the review lane's
scope meaningful for something other than a single file.

**Merge** is last because it is the only one that has to reconcile two
independently-created trees, and because `project.lifecycle.merge-identity`
requires a link sent a week earlier to still resolve — which means merge
cannot be implemented as "copy one into the other and delete".

## Three questions to answer first

*(All three answered; the first two are recorded in the spec under
§ Decided, the third remains open. Kept here because the reasoning about
what each one blocks is what made them worth asking before writing
code.)*

The spec records these as open. Two of them block slice one.

**Is "part" the right word?** It has to cover a song, a scene and an
episode without being a project. It appears in the lane's verbs, the
frontmatter key and every error message, so renaming it later is a
migration. *Blocks slice one.*

**Are a form's expected parts ever created automatically?** An `album`
expects songs; adopting fifteen song folders could propose fifteen
parts. Propose, create, or wait to be told — this changes what
`add_part` is for and whether adoption touches the project lane at all.
*Blocks slice one.*

**How do two sessions for one song relate?** A song can carry both a Pro
Tools and a Reaper session, which the real archive does. Whether they
need relating — same work, different tool — or merely co-exist. *Blocks
components, not parts.*

## What this does not propose

Building the whole spec. Sixteen scenario stages is a lot of surface,
and the first two slices unblock four of them while making the shape
concrete enough to judge the rest against. The `files.*` half of the
system got built roughly this way — a rule at a time, each with a
chapter — and it is the half that now has 55 verified rules and one
uncovered.
