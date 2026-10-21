# Release workflow (agents)

The procedure is `skills/release-workflow.md`; the logic is
`scripts/release.sh`, run through `just release-*`. This page is the
one-screen version for an agent that owns a promotion or a release.

## The shape: two stages

- Cards squash into `dev`. **Dev never freezes.** No CI on card PRs.
- **Promotion = `dev -> main`**: `just release-promote`. Full `checks` + the
  release guard run; the **Reviewer** reviews the diff and posts the review as a
  comment on the PR (code review happens
  here: "review at promotion to main"). Merge commit. **A push to main
  deploys nothing and uploads nothing to TestFlight.** `main` = finished
  features waiting for a release. Promote whenever a feature set is verified.
- **Release = a tag `vX.Y.Z` on main.** `just release-prepare` cuts
  `release/vX.Y.Z` from main (version bump + changelog, one commit),
  `just release-pr` opens it (guard only; `checks` is skipped for a
  version-only commit), merge commit, then `just release-tag`.
  **The tag deploys**: `deploy.yml` (server + web, Verify live), GitHub
  Release, `node-binaries.yml`, `task-ios.yml`, `task-macos.yml`
  (TestFlight, `desktop-v*`). No Captain approval for a release; patch bumps
  unless the Captain or Commander calls a minor/major.
- `just release-tag` ends by opening `backmerge/vX.Y.Z -> dev` (merge commit,
  auto-merge). Until it lands the next promotion is refused.
- **Production = the latest `v*` tag.**

## Commands

```bash
just release-plan          # live tag, main since the tag, waiting on dev, unreleased changelog, migrations
just release-promote       # open the dev -> main promotion PR
just release-prepare       # cut release/v<next> from origin/main (+ bump, changelog, release commit)
just release-pr            # push the release/hotfix branch, open its PR into main
just release-tag           # after the release PR merged: tag vX.Y.Z on main (DEPLOYS), then back-merge
just release-backmerge     # main -> dev, alone (after a hotfix); a no-op when dev has main's content
just release-hotfix NAME   # hotfix/NAME from the latest tag
```

## What the guard checks (release.yml, on every PR into main)

| Head | Fails when |
|---|---|
| `dev` (promotion) | nothing to promote; the version differs from main's (versions move only on main); the merge would not reproduce dev's tree (main has a release commit or hotfix dev lacks: back-merge first) |
| `release/vX.Y.Z` | the name differs from Cargo.toml's version; the version is not greater than main's or the tag exists; no CHANGELOG section; the head is not `chore(release): vX.Y.Z`; main is not in the branch; it is more than one commit; it changes anything but the version line(s) and CHANGELOG.md |
| `hotfix/*` | it bumps the version; it adds nothing; it conflicts with main |
| anything else | always (card PRs go to `dev`) |

## What the tag guard checks (deploy.yml, first step)

The ref is a `v*` tag, equal to `v<Cargo.toml version>`, on a commit that is in
main's history and whose CHANGELOG.md has that version's section. A branch
ref (`workflow_dispatch` without a tag) is refused: production changes only
by tag. To redeploy or roll back, run the workflow with the tag as its ref.

## Hotfix

`just release-hotfix <name>` branches from the latest tag. Fix, commit,
`just release-pr` (full checks), merge commit, then the normal release
(prepare, pr, merge, tag). Finished features promoted since the tag ship with
it. The back-merge brings the fix into dev.

## Branch protection

`main`: ruleset requiring a PR (merge commits only), `Checks (fmt + tests +
clippy + wasm + docs)` and `Release guard`; no direct pushes, no force-push,
no deletion; repository admins bypass. `dev`: ruleset requiring a PR and the
`Conventional PR title` check (no heavy checks: the card gate stays local).
fts-forge opens and merges the PRs.

## The sandbox test

`scripts/release.sh` was exercised end to end against a local bare clone of the
real `main`/`dev` (stub `cargo`/`gh`): the promotion guard (accepts a clean
promotion; refuses version bumps and un-back-merged main content), prepare from
main, every release-guard failure, the release-only detector, tag + tag guard +
publish, the back-merge no-op after a plain promotion and a real back-merge
after a release, a hotfix from the tag, and the 0.1.9 -> 0.1.10 -> 0.1.11
rollover.
