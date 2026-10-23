---
name: release-workflow
description: How a change reaches production in Task (fleet flow v4). Branch from main, PR into main with a cargo-rail change file, squash; a RELEASE is a CalVer tag vYY.M.MICRO (`just release-prepare` + release PR + `just release-tag`) and the tag alone deploys, uploads TestFlight and publishes the GitHub Release. Task's specifics only; the fleet `release-workflow` skill is the source of truth. Use when opening, reviewing or merging any PR in this repo, when cutting a release or a hotfix, and when rolling back.
---

# Release workflow in Task (fleet flow v4)

**The rules live in the fleet `release-workflow` skill** (fleet-skills,
`skills/release-workflow/SKILL.md` and `references/`: conventions, change
files and changelog, CI gates, cargo-rail). This page only adds what is
specific to Task. If the two disagree, the fleet skill wins and this page
is stale: fix it.

```
feat/sta-123-slug --PR (checks + review)--squash--> main --release PR--> tag v26.10.0 --> deploy
                                                     |  full suite after every merge
hotfix while main can't ship: release/v<latest tag> <-- cherry-pick -x from main --> tag (next CalVer)
```

## What runs where

| Event | What runs |
|---|---|
| push of a PR branch | the pre-push hook: `just ci-local` (fmt, change files, and the clippy / tests / wasm that `cargo rail plan` from the merge-base says are affected) on the local warm tree; when green it posts the **`local-ci`** status (required) on the pushed commits. Without the hook: `gh workflow run checks.yml --ref <branch>` posts it |
| PR into `main` | `PR` (pr.yml), job **`PR checks`** (required): a minimal free hosted backstop on rustup, no nix (fmt, `cargo rail change check`, rail route cases; the static guards run in `just ci-local`); plus **`local-ci`** (required); `Release guard` (required; passes at once unless the head is `release/pending`) |
| PR `release/pending` → `main` (one commit) | release guard (from the BASE commit's `scripts/release.sh`); `CI` skips the heavy jobs (version + changelog only) |
| PR into `release/v*` (hotfix) | the FULL `CI`, release guard |
| push to `main` or `release/v*` | FULL `CI` (lint and tests on WarpBuild, from their main-saved snapshots) + rail evidence for the commit; `release-tag` needs its aggregator green. If the push is a release commit (the version changed): `images` builds and pushes `ghcr.io/fasttrackstudios/task-{server,web}:<version>`; `release-tag` needs those too |
| nightly 10:17 UTC | FULL `CI` on `main`, unless a push run already passed at its head |
| tag `vYY.M.MICRO` | starcommand's Argo `task` app deploys the chart at the newest tag (images `:<appVersion>` = `:<version>`); `deploy` (tag guard → images on GHCR? → Verify live → GitHub Release → node binaries, desktop cache), `task-ios`, `task-macos` (TestFlight, `desktop-v*`) |

Nothing deploys on a merge. Production is the latest `v*` tag.

## Shipping a change

```bash
git fetch origin && git switch -c fix/sta-123-sync-stall origin/main
# … edit; build narrowest-first under the lease (rust-build-efficiency) …
cargo rail plan --explain                     # which tests CI will run
cargo rail change add files-store --bump patch -m "Fixed sync stalling after offline edits"
just ci-changes                               # the same change check CI runs
gh pr create --draft --base main --title "fix(files): stop sync stalling after offline edits"
```

Fill in `.github/pull_request_template.md`. Mark ready when `CI` is
green; squash-merge after review (`gh pr merge --squash --auto`). Change
files: `.changes/README`. A local `just ci` is optional, never the gate.

## Cutting a release (no Captain approval)

```bash
just release-plan       # latest tag, commits and change files since, next version, migrations
just release-prepare    # release/pending from origin/main: ONE commit chore(release): vYY.M.MICRO
just release-pr         # pushes it, opens the PR into main (body: changelog, migrations, git deps)
# squash-merge it while main has not moved (else: close, delete the branch, prepare again)
just release-tag        # once the full suite on the merge commit is green: tags it. THE TAG DEPLOYS.
```

- The version is computed (CalVer rule in the fleet skill); 0.2.4 was the
  last SemVer release, so the first v4 release is `26.<month>.0`. An
  explicit `just release-prepare X.Y.Z` must sort above the latest tag and
  leaves a `Release-Version-Override:` trailer the guard accepts.
- The CHANGELOG section is built from `.changes/*.md` (major → Breaking /
  Migration, minor → Added, patch → Fixed and improved, none skipped);
  with no change files at all it falls back to git-cliff (`cliff.toml`)
  over the commit range. Tidy wording by amending the one commit.
- `release-tag` refuses unless the commit is the release commit, has no
  change file left pending (if one is, main moved under the release PR and
  that change would ship unannounced: cut a new release), and its
  `CI passed` check is green. The tag must be pushed by a person or fts-forge,
  never a workflow `GITHUB_TOKEN` (that starts no workflow).

## Hotfix

1. Fix on `main` first (a normal PR with its change file).
2. If `main` is shippable, cut a normal release. Otherwise:

```bash
just release-hotfix sta-130-sync-stall <sha on main>…   # release/v<latest> from the tag, fix/<name> cherry-picked (-x), PR into it
# squash-merge it once the full suite is green, then:
just release-prepare --on release/v26.10.3              # next CalVer on the hotfix branch
just release-pr                                         # PR release/pending -> release/v26.10.3
just release-tag                                        # tags it there, then opens the forward-port PR into main
```

The forward-port PR copies the section into `main`'s CHANGELOG.md and
deletes the consumed change files there. `release/*` is never merged back.

## Task specifics

- **One version** for everything: `[workspace.package] version` in the root
  `Cargo.toml`. Server and web read `CARGO_PKG_VERSION`; the Apple
  workflows read `just version`; `desktop-v<version>` (the macOS in-app
  updater's feed) uses the same number. `just ci-versions`
  (`scripts/check-versions.sh`) fails when an artifact takes its version
  from anywhere else. Every comparison is numeric per component (release.sh
  `sort -V`, `apps/desktop/src/updater.rs`, Apple), so `26.10.0 > 0.2.4`.
- **Migrations** (sea-orm modules in `migrations.rs`) and moved git deps
  are listed by `just release-plan`, the release PR and the GitHub Release:
  a rollback past them is a data restore, not an image swap.
- **What is live**: `git tag --list 'v*' --sort=-v:refname | head -1`,
  `just release-plan`, or `https://task.starcommand.live/.well-known/task-server.json`
  (`build` is the deployed commit).

## Rollback

Get the Captain's yes/no first (target version, what restarts).
starcommand's Argo `task` app follows the NEWEST `v*` tag, so production
moves back by releasing a revert (or, in an emergency, by pinning that
app's `targetRevision` to `v<previous>` in starcommand; its images
`ghcr.io/fasttrackstudios/task-*:<previous>` stay on GHCR). A release that
carried migrations needs the pre-release data backup restored, or a forward
fix as a hotfix. Never move or delete a tag; fix forward and release a new
version.

## Don'ts

- Don't push to `main` or `release/*`, force-push them, or push a `v*` tag
  except through `just release-tag`.
- Don't edit CHANGELOG.md outside a release PR; the change file is the note.
- Don't use GitHub's skip tokens (the bracketed skip-ci family) in commit
  messages: they suppress `deploy.yml` too.
