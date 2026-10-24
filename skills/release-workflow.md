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
| push of a PR branch | the pre-push hook: `just ci-local` (fmt, change files, and the clippy / tests / wasm that `cargo rail plan` from the merge-base says are affected) on the local warm tree; when green it posts the **`Local CI`** status (required) on the pushed commits. Without the hook: `gh workflow run checks.yml --ref <branch>` posts it |
| PR into `main` | `PR` (pr.yml), job **`PR checks`** (required): a minimal free hosted backstop on rustup, no nix (fmt, `cargo rail change check`, rail route cases; the static guards run in `just ci-local`); plus **`Local CI`** (required); `Release guard` (required; passes at once unless the head is `release/pending`) |
| PR `release/pending` → `main` (one commit) | release guard (from the BASE commit's `scripts/release.sh`); `CI` skips the heavy jobs (version + changelog only) |
| PR into `release/v*` (hotfix) | the FULL `CI`, release guard |
| push to `main` or `release/v*` | FULL `CI` on THEBATTLESHIP's self-hosted `task-main` pool (warm dev target dirs; a newer merge cancels the older run and Tests narrow by the pool's rail evidence; a release commit's run is never cancelled and runs every test) + rail evidence for the commit; `release-tag` needs its aggregator green. If the push is a release commit (the version changed): `images` builds and pushes `ghcr.io/fasttrackstudios/task-{server,web}:<version>`; `release-tag` needs those too |
| tag `vYY.M.MICRO` | starcommand's Argo `task` app deploys the chart at the newest tag (images `:<appVersion>` = `:<version>`); `deploy` (tag guard → images on GHCR? → Verify live → GitHub Release → attaches node binaries, web bundle + Linux AppImage; desktop cache), `task-ios`, `task-macos` (TestFlight; the macOS app attached to the same release). One `v*` release carries everything the version ships |

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
just release-pr         # pushes it (Local CI vouches for a release commit at once, no build), opens the PR into main (body: changelog, migrations, git deps)
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

- **One release carries everything** the version ships: `task-server-*` +
  `SHA256SUMS{,.minisig}` (node-binaries.yml), `task-web-<v>.tar.gz` and
  `Task-linux-x86_64.AppImage` + `latest-linux.json` (release-assets.yml),
  `Task-macos.{dmg,zip}` + `latest-macos.json` (task-macos.yml), and the
  GHCR images named in the notes. The desktop updater reads only its
  platform's manifest, so a release whose build is still uploading is
  skipped until it lands. Every asset job is a no-op when the release has
  its asset, and backfills an existing tag from main:
  `gh workflow run release-assets.yml --ref main -f tag=vX.Y.Z`,
  `gh workflow run node-binaries.yml --ref main -f tag=vX.Y.Z`,
  `gh workflow run task-macos.yml --ref main -f desktop_release=publish -f tag=vX.Y.Z -f testflight=false`.
  Before 26.10 the macOS app had its own `desktop-v*` release; the updater
  still reads those.
- **One version** for everything: `[workspace.package] version` in the root
  `Cargo.toml`. Server and web read `CARGO_PKG_VERSION`; the Apple
  workflows read `just version`; the desktop apps on the release (and the
  `latest-macos.json` / `latest-linux.json` manifests the in-app updater
  reads) carry the same number. `just ci-versions`
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
