# Release workflow (agents)

The procedure is `skills/release-workflow.md`; the logic is
`scripts/release.sh`, run through `just release-*`. This page is the
one-screen version for an agent that owns a release.

## The shape

- Cards squash into `dev`. **Dev never freezes.**
- A release is `release/vX.Y.Z`, cut from dev's head by
  `just release-prepare` (patch by default; minor/major only when the
  Captain or Commander calls one). Cards that land on dev after the cut
  ship in the next release.
- `just release-pr` opens `release/vX.Y.Z -> main`. Full `checks` and the
  release guard run there. Merge commit, by the Captain (or his delegate).
- The push to main deploys, verifies live, tags `vX.Y.Z` on the release
  branch tip and publishes the GitHub Release; iOS/macOS ship X.Y.Z.
- `just release-backmerge` then opens `backmerge/vX.Y.Z -> dev` (merge
  commit, auto-merge). The next `release-prepare` refuses until it is in.

## Commands

```bash
just release-plan          # next version, unreleased changelog, migrations, git deps moved
just release-prepare       # cut release/v<next> from origin/dev (+ bump, changelog, release commit)
just release-pr            # push the release/hotfix branch, open its PR into main
just release-refresh       # on release/v*, after a fix PR landed on it
just release-backmerge     # after the release is on main: main -> dev
just release-hotfix        # on hotfix/* from main, only when no release branch is open
```

## What the guard checks (release.yml, on every PR into main)

| Check | Fails when |
|---|---|
| head ref | not `release/v<its Cargo.toml version>` and not `hotfix/*` (so `dev -> main` is refused) |
| cut point | the parent of the branch's first `chore(release): vX.Y.Z` commit is not in `origin/dev` |
| version | not X.Y.Z, not greater than main's, or the tag exists |
| changelog | CHANGELOG.md at the head has no `[X.Y.Z]` section |
| head commit | is not `chore(release): vX.Y.Z` (a fix landed after it: `just release-refresh`) |
| main in head | main has commits the branch lacks (back-merge first, or merge main into the branch and refresh) |
| tree identity | merging would not reproduce the head's tree |

It never compares with dev's head.

## Fixes during a release

- A release branch is open: PR the fix **into `release/vX.Y.Z`**, then
  `just release-refresh` and push. It reaches dev with the back-merge.
- None open: `hotfix/<what>` from `origin/main`, `just release-hotfix`,
  `just release-pr`, merge commit, `just release-backmerge`.

## The sandbox test

The flow was exercised end to end against a local bare clone of the real
`main`/`dev` (stub `cargo`/`gh`): the back-merge gate, prepare while dev
moves, every guard failure above, a fix on the release branch with refresh,
merge + publish (the tag lands on the release tip), back-merge, the next
release's changelog holding exactly the cards that landed during the
previous release, and a `hotfix/*` that does not ship dev's newer cards.
