---
name: release-workflow
description: How a change reaches production in Task — two separate stages. Card PRs squash into `dev` on the worker's own evidence (no CI there); `dev -> main` is a PROMOTION PR ("finished features": full checks + Reviewer on the diff, merge commit, nothing deploys); a RELEASE is a `vX.Y.Z` tag on main (`just release-prepare` + release PR + `just release-tag`) and the tag alone deploys, uploads TestFlight and publishes the GitHub Release. Use when opening, reviewing or merging any PR in this repo, when promoting, cutting a release or a hotfix, and when rolling back.
---

# Release workflow — main is finished features, a tag is a release

Features pile up on `main` before a release is cut. The two stages are
separate (the Captain, 2026-10-21):

```
card PR --squash--> dev --PROMOTION PR (merge commit)--> main        --release PR--> tag vX.Y.Z
 worker's own gate   │    full checks + Reviewer on the diff  │   version+changelog      │
                     │    NOTHING deploys                     │   one commit             ▼
                     │                                        │              deploy (server+web) → Verify live
                     │                                        │              task-ios / task-macos (TestFlight)
                     └◀── back-merge main -> dev ◀────────────┘              node binaries, GitHub Release
```

- **`dev`** collects work, including experimental changes. Dev never freezes.
- **`main`** is *finished features*: whatever was verified on its way in and is
  waiting for the next release. **A push to main deploys nothing and builds
  nothing for TestFlight.** Promotions happen whenever a feature set is verified.
- **A release is a tag `vX.Y.Z` on main.** The tag triggers `deploy.yml`
  (release build + deploy + Verify live), `task-macos.yml` / `task-ios.yml`
  (desktop + TestFlight), the GitHub Release and `node-binaries.yml`.
  **Production = the latest `v*` tag** (`just release-plan` prints it; the
  `task-production` GitHub environment shows the live sha).

| Where | What runs | Who merges | Merge method |
|---|---|---|---|
| PR `card -> dev` | `pr-title` only (a regex on a hosted runner). **No build.** | reviewer bot, on the card's evidence | **Squash** (PR title = the one commit = the changelog line) |
| `dev` nightly (10:17 UTC) | full `checks` on dev's tip | — | — |
| **PR `dev -> main` (promotion)** | full `checks` (no change selection, + rustdoc) and the **release guard**; the **Reviewer** agent reviews the diff | DevOps / Commander after Reviewer approval and green checks | **Create a merge commit** — never squash/rebase |
| push to `main` | **nothing** | — | — |
| PR `release/vX.Y.Z -> main` (version + changelog, one commit) | the **release guard**; `checks` is skipped (it only proves nothing else changed) | DevOps (no Captain approval) | **Create a merge commit** |
| push of tag `vX.Y.Z` (`just release-tag`) | `deploy` → Verify live → GitHub Release → node binaries; `task-ios`, `task-macos` | — | — |
| PR `backmerge/vX.Y.Z -> dev` | `pr-title` | auto-merge (opened by `just release-tag` / `release-backmerge`) | **Merge commit** |
| PR `hotfix/* -> main` | full `checks` + release guard | DevOps | Merge commit |

## For workers: shipping a card

1. Branch from **`origin/dev`** (not main) in the host's canonical checkout,
   holding the build lease (see the commander's rust-build-efficiency skill).
2. Build narrowest-first, then run the gate once on the final tree:

   ```bash
   git fetch origin && git rebase origin/dev     # the gate must cover what dev will get
   just ci 2>&1 | tee "$SCRATCH/just-ci.log"     # fmt, manifests, nextest, clippy, wasm
   ```

3. Record the evidence **on the card** (`kanban_comment`), all four lines:
   - `head:` the full sha you tested (`git rev-parse HEAD`)
   - `base:` the `origin/dev` sha you rebased onto
   - `just ci:` pass, with the nextest summary line (`N tests run: N passed`)
   - `host:` battleship / airlock and the lease owner
4. Open the PR **into dev** with a **conventional-commit title** — it becomes
   the squash commit and the changelog line:

   ```bash
   gh pr create --base dev --title "feat(files): a device that syncs the audio and leaves the footage" --body "Card: <id>. just ci passed on <sha> (base <dev sha>)."
   ```

   Types: `feat fix perf refactor docs test build ci chore revert style`;
   `!` before the colon marks a breaking change (`feat(api)!: …`). Scope is
   lower-case (`files`, `server`, `web`, `ios`, `deps`).
5. Do **not** wait on CI: there is none for this PR beyond the title check.
   Do not push again after recording the evidence. If you must, re-run
   `just ci` and record the new head.

### A branch cut before `dev` existed (from main or capture-process)

`dev` was created from main `b0750f0` on 2026-10-20, and `capture-process` was
deleted. Its content reached main as a squash, so its history is not an ancestor
of dev. A card branch cut from either base must be rebased so that only its own
commits move:

```bash
git fetch origin
git log --oneline origin/dev..HEAD           # more commits than yours? then:
git rebase --onto origin/dev <old-base>       # <old-base>: the last commit that is NOT yours
git range-diff <old-base>..@{1} origin/dev..HEAD  # the same patches, nothing else
just ci                                      # re-run on the rebased tree; record the new head
gh pr edit <N> --base dev                    # if the PR still targets main
```

Never force-push, not even your own branch. Push the rebased tree as
a new branch (`<branch>-dev`), open the PR into dev from it, and close the old PR.

## For the reviewer: merging a card into dev, and reviewing a promotion

Merge only when **all** hold; otherwise request changes naming the missing one:

- The PR's base is `dev`. (A card PR into `main` is refused: retarget it.)
- `pr-title` is green.
- The card carries the evidence above, and its `head:` equals the PR's head
  sha (`gh pr view <n> --json headRefOid`). A different sha means the
  evidence is for another tree.
- The evidence's `base:` is an ancestor of the current `origin/dev`, and dev
  has not moved in a way that touches the same crates. If dev moved a lot,
  ask the worker to rebase and re-run `just ci` — a pass on a stale base is a
  claim about a different merge.
- The changes are what the card asked for (normal review).

Then `gh pr merge <n> --squash` (keep the PR title as the subject). Need a
CI run anyway (an unfamiliar host, a flake you want reproduced)?
`gh workflow run checks.yml --ref <branch> -f base=dev` checks only the crates
the branch affects, on the warm runner.

**Reviewing a promotion (`dev -> main`)** is where the code review happens:
read the PR's full diff (`gh pr diff <n>`), classify findings as blocking /
should-fix / nit, and confirm `checks` and the release guard are green. A
blocking finding is fixed by a squash PR into dev; the promotion PR picks it
up. Approve in the Paperclip issue; the merge is a merge commit.

## Promoting dev to main (finished features)

When a feature set is verified on dev:

```bash
just release-plan              # what waits on dev, what main holds since the last tag
just release-promote           # opens "Promote dev to main (N changes)" with the commit list, migrations, git deps moved
```

- The PR runs the full `checks` on THEBATTLESHIP and the release guard
  (head is `dev`, the merge reproduces dev's tree, the version is untouched).
  If the guard says main has content dev lacks, a release or hotfix has not
  been back-merged: `just release-backmerge`, let it merge, re-run.
- **Code review happens here** (Captain, 2026-10-21: "review at promotion to
  main"): the **Reviewer** agent reads the PR's full diff (blocking /
  should-fix / nit) before merge. Card PRs into dev get no per-task review.
  Blocking findings are fixed as squash PRs into dev, and the promotion PR
  picks them up.
- Merge with **Create a merge commit** (subject `Promote dev to main (#N)`).
  Nothing deploys. dev needs no back-merge afterwards (the merge commit's
  tree is dev's tree).

## Cutting a release (DevOps; no Captain approval)

Whenever main holds enough finished features. Patch bump unless the Captain
or Commander called a minor/major.

```bash
just release-plan              # the unreleased changelog, migrations, git deps moved
just release-prepare           # patch; minor | major | X.Y.Z only when called (auto is opt-in)
#  -> release/vX.Y.Z cut from origin/main: Cargo.toml [workspace.package] version, Cargo.lock,
#     CHANGELOG.md's new [X.Y.Z] section, ONE commit "chore(release): vX.Y.Z".
just release-pr                # pushes it, opens "Release vX.Y.Z" -> main with notes + migration warnings
```

The release guard accepts the PR only when the branch is exactly one commit on
top of main that changes the version line(s) and CHANGELOG.md, nothing else
(the content was checked when it was promoted, so `checks` is skipped). If
main moves before the merge (another promotion), close the PR, delete the
branch and cut again.

Merge it with **Create a merge commit**. Then:

```bash
just release-tag               # tags vX.Y.Z on main (the release commit's merge), pushes it, opens the back-merge
```

**The tag is what deploys.** It must be pushed by a person or the fts-forge
bot: a tag pushed with a workflow's `GITHUB_TOKEN` starts no workflow.
`deploy.yml` first runs the tag guard (the tag is `v<Cargo version>` on a
commit of main with that CHANGELOG section), builds the tagged commit, pushes
images tagged `sha-<short>` and `vX.Y.Z`, waits for Verify live, then
`release.yml` publishes the GitHub Release and `node-binaries.yml` attaches
the standalone headless node binaries (docs/headless-node.md);
`task-ios` and `task-macos` upload TestFlight builds with marketing version
X.Y.Z and `task-macos` publishes `desktop-vX.Y.Z`.

The back-merge (`main -> dev`, `backmerge/vX.Y.Z`, merge commit, auto-merge)
is how dev gets the version bump and the changelog. It is a bot PR because a
PR opened with a workflow's `GITHUB_TOKEN` starts no workflows, so dev's
required title check would never report. If it conflicts: `git switch
backmerge/vX.Y.Z && git merge origin/dev`, resolve (`cargo update
--workspace`), push the branch. Until it merges, the next promotion PR is
refused by the guard.

### Knowing what is live

Production is the **latest `v*` tag**: `git tag --list 'v*' --sort=-v:refname | head -1`,
`just release-plan`, or `https://task.starcommand.live/.well-known/task-server.json`
(`build` is the deployed commit). `main` may be ahead of it by any number of
finished features. Nothing else deploys.

### The changelog

Each release's section is generated by git-cliff over a **range** — the
commits reachable from the release commit and not from the previous `v*` tag —
and written above the older sections. Wrong line? It is a wrong commit
subject; fix the PR title on the next card. Never hand-edit CHANGELOG.md.

### Versioning

- One version for the product: `[workspace.package] version` in the root
  `Cargo.toml`. Server and web read it as `CARGO_PKG_VERSION`; the Apple
  workflows read it through `just version`. `desktop-v*` tags (the macOS
  in-app updater's feed, `just release-desktop`) use the same number.
  Every artifact reports it — `task-server --version`, `task --version`,
  `fts-files-daemon --version`, `task-app-desktop --version`, the iOS and
  macOS marketing versions, the `v<version>` image tag and the Nix
  packages — and `just ci-versions` (scripts/check-versions.sh, in
  `just ci` and the release checks) fails when one takes its version from
  anywhere else; the deploy and node-binaries.yml run it with `--bin` on
  the binaries they built.
- **Patch bumps only** (v0.2.2, v0.2.3, …) until the Captain or Commander
  explicitly calls a minor or a major. `just release-prepare` bumps the
  patch whatever the commits say: a `feat` or a breaking change does not
  move the minor on its own. When one is called: `just release-prepare
  minor` / `major` / `X.Y.Z`. `just release-prepare auto` is the old
  behaviour (git-cliff decides from the commits) and is opt-in only.
  The patch number **never rolls over**: 0.1.9 → 0.1.10 → … → 0.1.86 → …
  for as long as needed. 0.2.0 comes only when it is called. Every version
  comparison is semantic, never a string compare: release.sh (`sort -V`,
  `git tag --sort=-v:refname`, an integer bump), the desktop updater
  (`apps/desktop/src/updater.rs` `newer`), and Apple's integer-per-component
  marketing version. So 0.1.10 > 0.1.9 everywhere.
  1.0.0 is a decision: `just release-prepare 1.0.0`.
- Product tags are exactly `vX.Y.Z`. `desktop-vX.Y.Z` is a separate feed and
  is ignored by the changelog.

## Hotfix

Branch from what production runs, PR it into main, then cut a patch release:

```bash
just release-hotfix <name>     # hotfix/<name> from the latest v* tag
# fix, commit (fix(scope): ...), just ci
just release-pr                # pushes hotfix/<name>, opens "Hotfix: <name>" -> main (full checks + guard)
# merge with a merge commit, then the normal release:
just release-prepare && just release-pr   # merge it
just release-tag               # tags the patch release; back-merge
```

Finished features promoted since the last tag ship with the hotfix: promoted
means verified. The hotfix does not bump the version; the release commit does.

## Rollback

- **Image-only release** (the release notes list no migrations and no moved
  git deps): redeploy the previous release — run the `deploy` workflow with the
  previous tag as its ref (`gh workflow run deploy.yml --ref v<previous>`), or
  roll the Argo app back to the images tagged `v<previous>` /
  `sha-<short>` in the registry. Then fix forward on dev, promote and release
  a patch so the latest tag is the good one.
- **Release with migrations**: an image rollback is not enough; the old
  binary does not know the new schema. Restore the pre-release data backup
  (taken before the release deploy) and accept loss of writes since; or
  ship a forward fix as a hotfix. The release PR body and the GitHub
  Release both carry the migrations list.
- Never delete or move a published `vX.Y.Z` tag. A bad release is
  superseded by the next one.
- Reverting this model itself (deploys on every push to main again) is one
  commit: restore the `on: push: branches: [main]` triggers of `deploy.yml`,
  `task-ios.yml`, `task-macos.yml` from the commit before the release-flow-v2 PR.

## Don'ts

- Don't open card PRs into `main`. Don't merge a promotion, release or
  hotfix PR into main with squash or rebase, or a back-merge into dev with
  squash (the guard and dev's ancestry depend on the merge commits).
- Don't push to `dev` or `main` directly; don't force-push either. Main
  is protected: PRs only, `Checks` and the `Release guard` required.
- Don't bump the version on dev. Versions move on main, in the release commit.
- Don't push a `v*` tag by hand except through `just release-tag`: it deploys
  to production. Pushing one on a card branch is refused by the tag guard.
- Don't use GitHub's skip tokens (the bracketed skip-ci family) in commit
  messages. There is no skip token any more.
- Don't run CI to "double check" a card that has evidence; it rebuilds the
  same tree in a second target dir.
