# The agent lane skills

A fork of [mattpocock/skills](https://github.com/mattpocock/skills),
pointed at Task instead of GitHub Issues.

Install into a working copy with:

```bash
task skills install
```

That writes the set with this org, project, verify command and runner
already filled in — which a generic installer cannot do, and is the
reason this is a CLI command rather than a file copy. The raw markdown
also works with `npx skills add`, unparameterised.

## What is forked and what is vendored

The tracker seam bites exactly six skills. Those are **rewritten**
against Task's verbs:

| Skill | Why it had to change |
|---|---|
| `setup` | replaced by `task skills install` |
| `triage` | four labels, not five; agents triage, so `needs-triage` is gone |
| `to-spec` | publishes with `task issue create` |
| `to-tickets` | blocking edges are `--blocker`, capabilities are `--cap` |
| `wayfinder` | maps and workstreams are Task entities |
| `implement` | the runner loop, not a bare commit |

Everything else is **vendored verbatim** — `grilling`, `tdd`,
`domain-modeling`, `code-review`, `research`, `prototype`,
`codebase-design`, `diagnosing-bugs`, `resolving-merge-conflicts`,
`wizard`, `handoff`, and the rest. Vendoring rather than depending on
the upstream plugin means one install produces one coherent set;
upstream's own README warns that installing both leaves you with every
skill twice.

## The rules the fork keeps

**Invocation.** A user-invoked skill may invoke model-invoked skills,
never another user-invoked one. Upstream states this and the fork does
not relax it.

**The tracker doc is the seam.** Skills consult
[ISSUE-TRACKER.md](ISSUE-TRACKER.md) rather than hard-coding verbs, so
retargeting is one file.

## The rules the fork changes

**Triage is agent-driven.** Upstream's five-state machine assumes a
maintainer evaluating inbound reports. Here agents triage everything
and surface what needs a person, so `needs-triage` and `needs-info`
are gone and `needs-input` / `needs-review` are added — a parked run
and a finished branch are different queues with different actions.

**A decision ticket is a workstream.** Upstream's map holds decision
tickets; here the ticket *is* the workstream entity, so it can own a
branch, carry a crew, and roll up.

**Done is an exit code.** Upstream leaves acceptance criteria as prose.
Anything an agent may take must resolve to a verify command, and is
refused the label otherwise.

## Keeping up with upstream

Vendored skills are copies. To see what has moved:

```bash
diff -ru ~/.claude/plugins/cache/claude-plugins-official/mattpocock-skills/*/skills \
         apps/task/skills/agent-lane
```

Rewritten skills will show large diffs by design; vendored ones should
show none until upstream changes.
