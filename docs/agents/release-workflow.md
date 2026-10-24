# Release workflow (agents)

Fleet flow v4. The rules are the fleet `release-workflow` skill (fleet-skills,
`skills/release-workflow/`); Task's specifics are `skills/release-workflow.md`;
the logic is `scripts/release.sh`, run through `just release-*`.

## The shape

- `main` is the only long-lived branch. Branch from `origin/main`, PR into
  `main` with a conventional title and a cargo-rail change file
  (`.changes/README`), squash. The full suite runs after every merge.
- **A release is a CalVer tag `vYY.M.MICRO`** on the release PR's squash
  commit (or on a `release/v*` hotfix branch). **The tag deploys**:
  `deploy.yml` (server + web, Verify live), the GitHub Release carrying
  everything (`node-binaries.yml`, `release-assets.yml`: web bundle + Linux
  AppImage), `task-ios.yml`, `task-macos.yml` (TestFlight + the macOS app
  on the same release). No Captain approval for a release.
- **Production = the latest `v*` tag.**

## Commands

```bash
just release-plan                     # latest tag, main since it, pending change files, next version, migrations
just release-prepare                  # release/pending from origin/main: version + CHANGELOG, ONE commit
just release-pr                       # push it, open the PR (squash)
just release-tag                      # after the merge, once its full suite is green: tag (DEPLOYS)
just release-hotfix NAME SHA...       # release/v<latest> + fix/NAME cherry-picked from main, PR into it
just release-prepare --on release/vX.Y.Z   # the hotfix's release commit
```

## What the guard checks (release.yml, from the BASE commit's script)

Only PRs whose head is `release/pending`; every other PR passes with a note.
It fails when: the PR is not exactly one commit on its base; the commit is
not `chore(release): v<version>`; it changes anything but the
`[workspace.package]` version line, workspace version lines in Cargo.lock,
CHANGELOG.md and deleted `.changes/*.md`; a change file is left pending; the
version is not the computed next CalVer (unless the commit carries a
`Release-Version-Override:` trailer and sorts above the latest tag); the tag
exists; the PR's base is not the target branch's CURRENT tip; or origin
cannot be reached.

## What the tag guard checks (deploy.yml, task-ios, task-macos)

The ref is a `vX.Y.Z` tag equal to `v<Cargo.toml version>`, on a commit in
the history of `main` or a `release/v*` branch, whose CHANGELOG.md has that
version's section. A branch ref is refused: production changes only by tag.
To re-verify or re-publish, run the workflow with the tag as its ref (Argo
deploys the newest tag; rolling back is a new release).

## Branch protection

`main`: squash-only PRs, required checks `PR checks` (pr.yml), `Release
guard` and `Local CI` (the pre-push hook's status); no direct pushes, no force-push, no deletion. Repository settings:
squash merges only, message = PR title + body, delete branch on merge,
auto-merge on.
