# Plans

Design docs for Task. Two directories, one rule.

## The convention

**Every plan carries a status header** — a `**Status:** …` line
immediately after the H1. No exceptions; a plan without one is a bug.
Use whichever of these is honest:

| Header | Means |
|---|---|
| `not started` | Written, nothing built. |
| `in progress` | Actively being built. |
| `partially shipped — needs triage (<date>)` | Some of it exists; nobody has checked which parts. **Say this rather than guessing.** |
| `shipped` | Done. Move the file to `done/`. |
| `superseded by <file>` | Replaced. Move the file to `done/`. |
| `abandoned` | Target architecture got ripped. Move the file to `done/`. |
| `parity tracker — ongoing` | Living scoreboard against another project. Never "done"; stays here. |
| `research` / `decision record` / `historical audit (<date>)` | Not a work item. Stays here as reference. |
| `unknown — needs triage (<date>)` | You genuinely can't tell. Better than a fabricated status. |

**Finished plans go in [`done/`](done/)** — that is the single
terminal directory. It holds both the *shipped* plans (with the design
rationale for what landed) and the *abandoned* ones (whose target
architecture was ripped out). Each file's own status header says which
it is. There is no separate `archived/` anymore — it was merged into
`done/` on 2026-07-27, because two terminal directories meant plans
drifted between them and neither got trusted.

**[`handoff/`](handoff/) is different and stays separate.** It is not
a terminal state — it holds session hand-off notes ("here is where I
stopped, here is what's next"), addressed to the next agent picking up
a specific arc. Plans describe *what to build*; handoffs describe
*where a particular run of the work stopped*.

## Reading order for a fresh agent

1. Top-level `plans/*.md` — active and reference work.
2. `done/` — only when you need the rationale behind something that
   already exists, or want to mine an abandoned design. **Do not read
   `done/` to learn what Task is today**; much of it targets
   architecture that no longer exists. In particular the Loro-entity
   layer (`EntityCrdt`, `*RepoLoro`, per-feature `-crdt` crates) was
   removed in full — see `done/project-crdt-rip.md` and
   `done/knowledge-rip.md`, and the abandoned designs that depended on
   it (`done/decentralized-foundation.md`,
   `done/derive-entity-crdt.md`, `done/sync-architecture.md`,
   `done/loro-text-editor-upgrade.md`, `done/cursor-awareness.md`,
   `done/logseq-*.md`, `done/threads-feature-deepening.*`,
   `done/vox-phase-{2,3}-*.md`, `done/agent-mvp.goal.md`,
   `done/agent-p4-dashboard.goal.md`, `done/notifications-mvp.goal.md`,
   `done/obsidian-vault-mount.md`, `done/vault-publisher.md`,
   `done/vertical-slice-surprises.md`).
3. [`../AGENTS.md`](../AGENTS.md) for the architecture that *is*
   current.

## When to add a plan here

- **Multi-slice work**: write it down before the second commit. Commit
  messages capture *what changed*; the plan captures *why this and not
  the alternatives*.
- **Research**: when you've spent a day reading another project's
  code, capture the model and what to crib vs not (see
  `knowledge-graph.md` for the shape).
- **Don't write plans for**: single-commit refactors, bug fixes,
  mechanical renames. The commit message is enough.

## Reviving something from `done/`

A design idea in `done/` may still be sound even when its
implementation path is stale. To bring one back: rewrite it against
the *current* substrate (file-backed `vault`, `#[architect::rpc]`
services, sea-orm for server-private and service tables) and put the
new version at the top level. The old file stays in `done/` as the
trail.
