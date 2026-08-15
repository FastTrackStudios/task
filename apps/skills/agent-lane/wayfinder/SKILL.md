---
name: wayfinder
description: Plan a chunk of work too big for one agent session as a map of workstreams in Task, and resolve them one at a time until the way to the destination is clear.
disable-model-invocation: true
---

A loose idea has arrived — too big for one session, and wrapped in
fog: the way from here to the **destination** isn't visible yet.
Wayfinding is about finding that way, not charging at the destination.

Read [../ISSUE-TRACKER.md](../ISSUE-TRACKER.md) first. Everything here
lives in Task.

## Plan, don't do

Each workstream resolves a decision. The map is done when nothing is
left to decide before someone goes and builds. The pull to just do the
work is usually the signal you've reached the edge of the map and it's
time to hand off.

## Refer by name

Every map and workstream has a title. In everything a human reads,
refer to it by that title, never by a bare id. A wall of `a7c62905,
dd1e00ab, ed88375c` is illegible; names read at a glance. The id rides
inside the name, never stands in for it.

## The map

An ordinary Task issue. Its body carries five sections, and the
sections round-trip losslessly — anything you add between them
survives, so hand edits are safe.

```markdown
## Destination

<what reaching the end looks like. One or two lines; every session
orients to it before choosing what to work.>

## Notes

<domain; skills to consult; standing preferences for this effort>

## Decisions so far

- [<closed workstream title>](<id>) — <one-line gist of the answer>

## Not yet specified

<in-scope fog you cannot state sharply enough to ticket yet>

## Out of scope

<work consciously ruled beyond the destination; never graduates>
```

The map is an **index, not a store**. A decision lives in exactly one
place — its workstream — and the map only gists it and links.
*Decisions so far* is append-only: never rewrite an entry, because
each one is the only pointer to a closed workstream's reasoning.

## Workstreams

What upstream calls a *decision ticket*. Each is one session's worth of
context, created as a subtask of the map:

```bash
task issue create "<the question>" --parent <map> --workstream <ws> \
  --body - <<'EOF'
## Question

<the decision or investigation this resolves>
EOF
```

Claim before any work — `task issue start <id> --as-agent <you>` —
so a concurrent session skips it. Wire blocking in a second pass, once
ids exist: `task issue relate <a> blocks <b>`.

The **frontier** is `task issue ready`: open, unblocked, unclaimed.

## Types

Every workstream is either **HITL** — worked *with* a human who speaks
for themselves — or **AFK**. A HITL workstream resolves only through
that live exchange. **The agent never stands in for the human's side
of it.** A grilling session that answers its own questions has broken
this, and so has an agent that decides a question is obvious.

- **Research** (AFK) — reading docs or code to surface a fact a
  decision waits on. Use `/research`.
- **Prototype** (HITL) — make a cheap concrete thing to react to when
  "how should it behave" is the question.
- **Grilling** (HITL) — conversation. The default. Use `/grilling` and
  `/domain-modeling`.
- **Task** (HITL or AFK) — manual work that must happen before a
  decision can be made. The one type that *does* rather than decides,
  and it earns its place by unblocking a decision.

## Fog of war

The map is deliberately incomplete. Beyond the live workstreams lies
fog: decisions you can tell are coming but cannot yet pin down.

**Fog or workstream?** Whether you can state the question precisely
now — *not* whether you can answer it. Ticket a sharp question even if
it is blocked. Leave a vague one in *Not yet specified*, and don't
pre-slice it: one patch of fog may graduate into several workstreams,
or none.

## Out of scope

Fog gathers only *toward* the destination. Work past it is out of
scope, not fog. When an existing workstream turns out to sit beyond the
destination, **close it** and leave one line under *Out of scope* with
the gist and why. It stays out of *Decisions so far*, which records the
route actually walked — a scope boundary is not a step on it.

## Invocation

Never resolve more than one workstream per session, research excepted.

### Chart the map

1. **Name the destination.** `/grilling` + `/domain-modeling`. The
   destination fixes the scope, so it is settled first.
2. **Map the frontier.** Grill again, breadth-first, across the whole
   space. **If no fog surfaces**, the way is already clear and you do
   not need a map — say so and stop.
3. **Create the map** with Destination and Notes filled, Decisions
   empty, the fog sketched into *Not yet specified*.
4. **Create the workstreams you can specify**, then wire blocking in a
   second pass.
5. **Fire the research subagents** for any research workstreams.
6. Stop. Charting is one session's work and resolves nothing.

### Work through the map

1. Load the map body — the low-res view, not every workstream.
2. Choose a workstream. Without one named, take the first on the
   frontier. **Claim it first.**
3. Resolve it, zooming into related closed workstreams on demand.
4. Post the answer, close it, append one line to *Decisions so far*.
5. Graduate any fog the answer sharpened, clearing it from *Not yet
   specified*. If the answer reveals something sits past the
   destination, rule it out of scope rather than resolving it.

Expect other sessions to be editing Task concurrently.
