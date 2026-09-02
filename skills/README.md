# Skills

Agent-callable recipes. Each skill is a standalone markdown file with:

- YAML frontmatter (`name`, `description`, optional `runs_as`, `trigger`)
- Prose describing when and how to run the skill, which CLI commands
  are used, and what decisions the agent needs to make

Hermes (the autonomous agent on starcommand) discovers and invokes
these skills. Skills are deliberately *declarative recipes*, not code:
they compose the `task` CLI (and any other shell tooling) and depend
on the CLI for idempotence + audit.

## Conventions

- One skill per file, kebab-case filename that matches the `name`
  frontmatter value (`email-triage.md` ↔ `name: email-triage`).
- Every command shown must be copy-paste runnable. Do not paraphrase
  CLI flags.
- Prefer idempotent command sequences. Every skill should be safe to
  re-run after partial failure.
- Document the expected `TASK_USER` / `NEXTCLOUD_USER` context — most
  skills run as a specific agent user (curator, researcher, jarvis)
  and rely on audit attribution.
- Call out decision boundaries explicitly (when to defer to Cody vs.
  when to decide unilaterally).

## Current skills

- [`live-dev.md`](live-dev.md) — the local web app against the
  deployed server and the real issuer, signed in as the account in
  `.env` (`just live`); the CLI as that account; the traps that make
  it look broken.
- [`email-triage.md`](email-triage.md) — curator sorts the agent@
  inbox, links to tasks/projects, applies Proton labels, marks
  processed.
- [`task-triage.md`](task-triage.md) — files unfiled tasks (no
  project / parent / workstream / `@context`) so they leave the
  triage strip and rejoin the Relevant view. Drives the MCP
  `list_untriaged_tasks` → `file_task` loop; also runnable from the
  CLI via `task task list --untriaged` + `task task set-project`.
  Scheduled from the Tasks UI routines panel ("Triage unfiled
  tasks" preset).
