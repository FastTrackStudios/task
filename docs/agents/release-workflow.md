# Release workflow (agents)

Tag-from-main, the same flow as studio's. Task's specifics are
`skills/release-workflow.md` (they win over the fleet `release-workflow`
skill, which still describes release PRs); the logic is
`scripts/release.sh`, run through `just release*`.

## The shape

- `main` is the only long-lived branch. Branch from `origin/main`, PR into
  `main` with a conventional title and a cargo-rail change file
  (`.changes/README`). A lone PR squash-merges; a stack merges with merge
  commits titled with each PR's title and body, so the first-parent
  history of `main` is one conventional commit per PR (`git log
  --first-parent`, `git bisect start --first-parent`). PRs merge in
  batches without waiting on CI per change; `main`'s full suite, which
  runs after the merges, is the gate, checked once per batch and fixed
  forward.
- **Release notes come from the change files, never from commits.**
- **A release is a CalVer tag `vYY.M.MICRO`** that `just release` pushes on
  `main`'s newest commit whose `CI passed` is green (or on a `release/v*`
  hotfix line). There is no release PR and no release commit: every
  release build stamps the tag's version into its own checkout
  (`release.sh stamp`). **The tag ships**: `deploy.yml` (Images, then
  production follows through argocd-image-updater; Verify live), the
  GitHub Release carrying everything (`node-binaries.yml`,
  `release-assets.yml`: web bundle + Linux AppImage), `task-ios.yml`,
  `task-macos.yml` (TestFlight + the macOS app on the same release). No
  Captain approval for a release.
- **Production = the latest `v*` tag.**

## Commands

```bash
just release-plan                     # latest tag, main since it, next version, draft notes, migrations
just release --dry-run                # the commit it would tag, the version, the notes; no tag
just release                          # tag main's newest green commit and push it (SHIPS)
just release <sha>                    # a given commit on main or release/v*, CI passed green
just release-hotfix NAME SHA...       # release/v<latest> + fix/NAME cherry-picked from main, PR into it
just release-notes [vX.Y.Z]           # a tag's notes, or the draft for origin/main
```

## What the tag guard checks (deploy.yml, task-ios, task-macos)

After `stamp`: the ref is a `vX.Y.Z` tag equal to the (stamped) `v<Cargo.toml
version>`, the checkout is that tag, the commit is in the history of
`main` or a `release/v*` branch, and its `CI passed` is green (with a
token). A branch ref is refused: production changes only by tag. To
re-verify or re-publish, run the workflow with the tag as its ref
(production runs the highest image; rolling back is a new release).

## Branch protection

`main`: PRs only, required check `PR checks` (pr.yml) only; `Local
CI` (the pre-push hook's status) is an indicator. No direct pushes, no
force-push, no deletion. Repository settings: squash merges (a lone PR)
and merge commits (a stack, `allow_merge_commit`), both titled with the
PR title and body (`merge_commit_title=PR_TITLE`,
`merge_commit_message=PR_BODY`), delete branch on merge, auto-merge on.
