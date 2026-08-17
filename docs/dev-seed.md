# The dev demo seed

`task-server admin seed` stands up a **throwaway local dataset** rich
enough to develop and screenshot every Files / Projects / performance
surface against — and deterministic enough that Playwright / dioxus
tests can assert on exact names. It is the standard way to get a dev
rig with real data; nothing in it is production.

## Quick start

```bash
# One-shot wrapper (build + seed + guardrails):
apps/task/scripts/dev-seed.sh seed     # or `fresh` to wipe first
apps/task/scripts/dev-seed.sh serve    # terminal 1: the server
apps/task/scripts/dev-seed.sh web      # terminal 2: dx serve, wired up

# Or drive the verb directly (TASK_DATA_ROOT is REQUIRED — the seed
# refuses to run against the default data root, because it plants a
# known-password admin):
TASK_DATA_ROOT=/tmp/task-dev ./target/debug/task-server admin seed
```

Sign in as `dev@fasttrackstudio.dev` / `password` (override with
`--email` / `--password`). The seed also plants the `DEV_ACCOUNTS`
roster the debug web build auto-boots with: `cody@fasttrackstudios.com`
(admin), `carter@…`, `tom@…`, `guest@…`, passwords `dev-<name>-2026`.

**Real media needs ffmpeg on PATH.** With it, every video project gets
real playable MP4s (testsrc, hue-varied per version so version compare
shows different pictures) and every song real WAVs — a fresh seed is
~9s (encodes run in parallel). Without it you get labeled placeholder
bytes: browsing works, playback and renditions don't, and the seed
prints a warning saying so.

`seed` is **dev-only**: the verb is `#[cfg(debug_assertions)]` and
does not exist in a release binary.

## What it plants

Three orgs (`fasttrackstudio` [home], `acme-films`, `northwind`; or
`--orgs a,b,c`). Every org gets the owner account, welcome notes, and
a "Demo Project" Files root (3 checkpoints, a Named Version, a
divergence — `--no-divergence` opts out everywhere, studio roots
included). The **home org additionally gets the studio dataset**:

- **50 projects** (`vault/Projects/<name>/` + `vault/Albums/<name>/`),
  each a folder with the project note (`type: project`, `kind:
  video|album|engagement`, statuses rotating through the real
  `ProjectStatus` variants active / on-hold / done / stale) plus
  `Notes/Brief.md` and `Notes/Session Log.md` — the project's
  vault-within-the-vault:
  - 10 video productions ("Aurora Sneaker Spot", "Hayes Wedding Film",
    "Skyline Doc — Ep 1", …),
  - 3 albums — Midnight Static (6 songs), Golden Hour (4), Roots &
    Wires (8) — whose songs are **sub-project folders** carrying a
    full `type: song` note (key / bpm / duration_sec / sections) and
    production notes,
  - 37 other engagements.
- **The performance stack**: 8 live songs in `Songs/`, two
  `type: setlist` notes (`Sunday Setlist`, `Album Release Show`), two
  `type: event` + `experience: setlist` events with start/end under
  `Records/events/` (`Sunday Service`, `Album Release Show — Fox
  Theater`). Basenames are unique vault-wide on purpose — wikilink
  resolution is basename-keyed.
- **13 media Files roots**: one per video project (2 checkpoints,
  `★ Rough Cut v1` on even indices, divergences on the first two —
  kept on `notes.md` so the cuts stay playable) and one per album
  (`Album — <name>`, per-song `mix.wav` + `stems/`, a named
  `Mix v1`). These join the org tree as each project's `Media/`.

## Guarantees tests can rely on

- **Deterministic names.** Every project, song, setlist, event, and
  root name above is fixed. Assert on `"Aurora Sneaker Spot"`,
  `"Album — Golden Hour"`, `"Golden Hour (Reprise)"` freely.
- **Idempotent AND healing.** Re-running tops up what's missing and
  otherwise no-ops (`0 planted, 0 topped up`): notes are
  write-if-absent; roots are dressed based on the probe file's chain
  length, so a run interrupted mid-encode completes on the next run
  instead of leaving a half-seeded root forever.
- **Safety rails.** Refuses to run without an explicit
  `TASK_DATA_ROOT`; compiled out of release builds.

## Gotchas

- **Never seed while a server holds the same data root** — stop it
  first (shared sqlite/jj stores). The dev-seed.sh wrapper sequences
  this for you.
- Wiping and reseeding **invalidates existing browser sessions** (new
  account ids) — sign in again.
- A root's **first browse after a server start is a cold open** (jj +
  blob store) and can take tens of seconds; subsequent browses are
  instant. Don't read a hang into the first skeleton screen.
- The seed's implementation (and its one-emitter rules: `song_note()`
  for `type: song` frontmatter, `demo_slug()` for root dirs) lives in
  `apps/task/server/src/admin_cli.rs` — extend the constants there
  and re-run; new entries plant, existing ones stay put.
