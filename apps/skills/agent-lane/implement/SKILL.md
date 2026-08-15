---
name: implement
description: Build the work a Task ticket describes — claim it, work it on its own branch, prove it with the verify command, and hand back a reviewable branch.
disable-model-invocation: true
---

Implement the ticket. Read [../ISSUE-TRACKER.md](../ISSUE-TRACKER.md)
first.

## Let the runner do it

If a runner is registered for this repo, you do not need to drive any
of this by hand:

```bash
task runner work --repo <path> --worktree-root <path> \
  --agent-cmd 'claude -p --permission-mode acceptEdits'
```

That claims a ticket, cuts a worktree and branch, runs the agent, runs
the verify command, commits, and moves the ticket to `needs-review`.
`task runner serve` does it continuously.

Everything below is what happens inside that loop — and what to do
when you are the agent inside it.

## Working a ticket by hand

1. **Claim before touching anything.**
   `task issue start <id> --as-agent <you>`. Claiming is atomic; if you
   lose, stop — someone else has it.

2. **Work on the ticket's own branch**, cut from the base. Never move
   the repository's checkout, and never push.

3. **Use `/tdd`** at the seams the spec agreed. Typecheck as you go,
   run single test files often, the full suite once at the end.

4. **Run the verify command.** Not a command you think is equivalent —
   the one the ticket resolves to. Exit zero is the only pass.

5. **`/code-review`** before committing.

6. **Commit to the branch.** The branch is the handback. Nothing
   merges to a mainline, and nothing is pushed.

## When you are stuck

Ask. Do not guess, and do not decide the question is obvious:

```bash
task runner ask <ticket> "<the question>" --option a --option b
```

That records the question and flips the ticket to `needs-input`, which
takes it out of the runner queue until a human answers. Answering
restores `ready-for-agent`.

**Never answer your own question.** A human-in-the-loop decision
resolves only through the human. If you find yourself reasoning about
which answer they would probably pick, that is the moment to ask.

## When you change nothing

Say so and make no commit. An empty commit makes the branch lie about
what happened.

## When verify fails

Leave the worktree. Do not delete evidence to make the next attempt
cleaner — the failed tree is what someone will look at. Release the
claim so the ticket can be retried; the attempt is kept either way, so
"this has died three times on the same command" stays answerable.
