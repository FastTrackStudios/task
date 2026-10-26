---
name: release-workflow
description: How a change reaches production in Task (tag-from-main, the same flow as studio's). Branch from main, PR into main with a cargo-rail change file, squash; a RELEASE is a CalVer tag vYY.M.MICRO that `just release` pushes on main's newest green commit. No release PR, no release commit; the tag alone builds the images (production follows), uploads TestFlight and publishes the GitHub Release. Task's specifics; they win over the fleet `release-workflow` skill where the two disagree. Use when opening, reviewing or merging any PR in this repo, when cutting a release or a hotfix, and when rolling back.
---

# Release workflow in Task (tag-from-main)

Task releases **from the tag**, exactly as studio does. Any commit on
`main` whose `CI passed` is green can be released by pushing a CalVer tag,
and the tag alone starts the release builds. There is no release PR and no
release commit. The fleet `release-workflow` skill holds the shared rules
(branch names, PR titles and template, change files, CalVer); it still
describes release PRs, so where it disagrees, this page wins.

```
feat/sta-123-slug --PR (PR checks, change file)--squash--> main -- CI (`CI passed`) after every merge
                      just release --> tag v26.10.2 on main's newest green commit
                                         | (the tag alone, deploy.yml + the Apple workflows:)
                                         |-- Images: ghcr.io/fasttrackstudios/task-{server,web}:26.10.2 --> image-updater --> production
                                         |-- GitHub Release (notes from .changes/) + node binaries, web bundle, Linux desktop
                                         '-- task-ios / task-macos: TestFlight, the macOS app on the same release
hotfix: release/v26.10.2 (from the tag) <-- cherry-pick -x PR -- CI passed -- just release <sha> --> v26.10.3
```

## What runs where

| Event | What runs |
|---|---|
| push of a PR branch | the pre-push hook: `just ci-local` (fmt, change files, and the clippy / tests / wasm that `cargo rail plan` from the merge-base says are affected) on the local warm tree; when green it posts the **`Local CI`** status on the pushed commits: an indicator, not required. Without the hook: `gh workflow run checks.yml --ref <branch>` posts it |
| PR into `main` | `PR` (pr.yml), job **`PR checks`**, the only required check: a minimal free hosted backstop on rustup, no nix (fmt, `cargo rail change check`, rail route cases; the static guards run in `just ci-local`) |
| PR into `release/v*` (hotfix) | the FULL `CI` |
| push to `main` or `release/v*` | FULL `CI` on THEBATTLESHIP's self-hosted `task-main` pool (warm dev target dirs; a newer merge cancels the older run, and Tests narrow by the pool's rail evidence) + rail evidence for the commit. `just release` tags only a commit whose `CI passed` is green. Nothing ships |
| tag `vYY.M.MICRO` | every job checks out the tag and runs `scripts/release.sh stamp` first, then the tag guard (vX.Y.Z on main or `release/v*`, `CI passed` green; the suite is not run again). `deploy`: Images (task-server + task-web `:X.Y.Z`, `sha-<short>`, `latest` for the highest, on the `task-release` pool) and, beside it, the GitHub Release → node binaries, and after the images the web bundle + Linux AppImage; Verify live (informational: waits for production to serve the tag, fails nothing). `task-ios`, `task-macos`: TestFlight, and the macOS app attached to the same release. One `v*` release carries everything the version ships |

Production is the latest `v*` tag: starcommand's argocd-image-updater
rolls the `task` app to the highest `X.Y.Z` image on GHCR (the chart,
`deploy/chart/task`, no longer carries the release: its `appVersion` stays
at 26.10.1, so a new tag's chart runs what is deployed until the images
land and the updater moves both). Nothing deploys on a merge.

## Shipping a change

```bash
git fetch origin && git switch -c fix/sta-123-sync-stall origin/main
# … edit; build narrowest-first under the lease (rust-build-efficiency) …
cargo rail plan --explain                     # which tests CI will run
cargo rail change add files-store --bump patch -m "Fixed sync stalling after offline edits"
just ci-changes                               # the same change check CI runs
gh pr create --draft --base main --title "fix(files): stop sync stalling after offline edits"
```

Fill in `.github/pull_request_template.md`. Squash-merge once `PR checks`
is green and it is reviewed (`gh pr merge --squash --auto`). Change files:
`.changes/README`. A local `just ci` is optional, never the gate.

## Cutting a release (no Captain approval)

```bash
just release-plan            # latest tag, what main holds since, next version, draft notes, migrations, moved git deps
just release --dry-run       # the commit it would tag (main's newest green), the version, the notes; no tag
just release                 # tag + push (asks first on a terminal; --yes skips it): THIS SHIPS
just release <sha>           # a given commit: on main or a release/v* line, with CI passed green
just release-notes [vX.Y.Z]  # a tag's notes (default: the draft for origin/main)
```

- `just release` walks `main` back from its tip (first parent, down to
  the previous tag) and takes the newest commit whose latest `CI passed`
  run succeeded, saying why it skipped each newer one. It refuses a commit
  without a green `CI passed`, one on neither `main` nor a `release/v*`
  line, one already tagged or older than its line's newest release, and a
  next version that is not above the highest tag.
- **The version is the tag.** It is computed, never chosen: the highest
  `vX.Y.Z` tag; same `YY.M` as this month (UTC) → `MICRO + 1`, else
  `YY.M.0`. Releases commit no version: every release build runs
  `release.sh stamp X.Y.Z` after its checkout, writing the tag's version
  into `[workspace.package] version` and the workspace members'
  `Cargo.lock` entries, uncommitted. **The version committed on `main`
  (26.10.1, the last release commit) is not the released version**;
  `just release-plan` shows both.
- **Release notes** come from the change files **added** since the
  previous tag, read at the tagged commit and grouped (major → Breaking /
  Migration, minor → Added, patch → Fixed and improved, none → nothing),
  plus the migrations and moved git deps. They go into the GitHub Release;
  nothing is deleted or committed, and the change files stay in
  `.changes/` as the record. `CHANGELOG.md` ends at 26.10.1 and is no
  longer written. To fix a note, edit the change file on `main` before
  releasing.
- Push the tag as a person or fts-forge, never a workflow `GITHUB_TOKEN`
  (that starts no workflow). Never move or delete a tag; a bad release is
  superseded by the next.

## Hotfix

1. Fix on `main` first (a normal PR with its change file).
2. If `main` is shippable, release it normally. Otherwise:

```bash
just release-hotfix sta-130-sync-stall <sha on main>…   # release/v<latest> from the tag, fix/<name> cherry-picked (-x), PR into it
# squash-merge it once its full suite is green; once release/v…'s CI passed is green:
just release <merge sha>                                # tags the next CalVer there
```

Nothing is forward-ported (a release commits nothing): `main` already has
the fix, and its next notes say "Includes all changes up to v…" without
listing the hotfixed change files again. `release/*` is never merged back.

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
  `gh workflow run task-macos.yml --ref main -f desktop_release=publish -f tag=vX.Y.Z -f testflight=false`,
  `gh workflow run images.yml --ref main -f tag=vX.Y.Z`.
  Before 26.10 the macOS app had its own `desktop-v*` release; the updater
  still reads those.
- **One version** for everything: `[workspace.package] version` in the root
  `Cargo.toml`, which a release build has stamped with the tag's. Server
  and web read `CARGO_PKG_VERSION`; the Apple workflows read `just
  version`; the desktop apps on the release (and the `latest-macos.json` /
  `latest-linux.json` manifests the in-app updater reads) carry the same
  number. `just ci-versions` (`scripts/check-versions.sh`) fails when an
  artifact takes its version from anywhere else, or a release workflow
  builds without stamping. Every comparison is numeric per component
  (release.sh `sort -V`, `apps/desktop/src/updater.rs`, Apple), so
  `26.10.0 > 0.2.4`.
- **Build revision**: a stamped checkout is dirty, so Nix has no
  `self.rev`; images.yml passes the tagged commit as `FTS_BUILD_REV`
  (`nix/modules/crane.nix`), which is what `/version.json`,
  `TASK_BUILD_REV` and the images' revision label report and Verify live
  compares.
- **Migrations** (sea-orm modules in `migrations.rs`) and moved git deps
  are listed by `just release-plan`, `just release` and the GitHub
  Release: a rollback past them is a data restore, not an image swap.
- **What is live**: `git tag --list 'v*' --sort=-v:refname | head -1`,
  `just release-plan`, or `https://task.starcommand.live/.well-known/task-server.json`
  (`build` is the deployed commit).

## Rollback

Get the Captain's yes/no first (target version, what restarts).
image-updater runs the HIGHEST `X.Y.Z` image, so production moves back by
releasing a revert (or, in an emergency, by pinning the `task` app's image
tags in starcommand; `ghcr.io/fasttrackstudios/task-*:<previous>` stays on
GHCR). A release that carried migrations needs the pre-release data backup
restored, or a forward fix as a hotfix. Never move or delete a tag; fix
forward and release a new version.

## Don'ts

- Don't push to `main` or `release/*`, force-push them, or push a `v*` tag
  except through `just release`.
- Don't edit CHANGELOG.md; the change file is the note.
- Don't use GitHub's skip tokens (the bracketed skip-ci family) in commit
  messages: they suppress `deploy.yml` too.
