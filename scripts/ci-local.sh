#!/usr/bin/env bash
# Local CI: the expensive half of the PR gate, run on this machine's warm
# target dir instead of a billed runner.
#
#   scripts/ci-local.sh [BASE]        run the checks (`just ci-local`)
#   scripts/ci-local.sh --post SHA…   mark each commit `local-ci` on GitHub
#                                     once GitHub has it (the pre-push hook
#                                     runs this detached, after the push)
#
# PRs into main run only the free hosted checks (checks.yml: fmt, change
# files, versions, manifests, boundaries) plus the `local-ci` status this
# posts. Clippy, the tests and the wasm check run HERE, for what the change
# affects: `cargo rail plan` from BASE (default: the merge-base with
# origin/main) through cargo's reverse-dependency graph, turned into gates
# by scripts/ci-rail-plan.py, the same script CI plans with. The full set
# runs on WarpBuild after every merge to main, which is what catches two
# PRs that pass alone and break together.
#
# Order is the Justfile's: the test build does the codegen clippy reuses.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if [ "${1:-}" = "--post" ]; then
  shift
  repo="$(gh repo view --json nameWithOwner -q .nameWithOwner)"
  desc="clippy + rail-planned tests passed on $(hostname -s)"
  for sha in "$@"; do
    # The hook runs before the push lands: wait for GitHub to know the
    # commit (a status on an unknown sha is a 422).
    for _ in $(seq 1 60); do
      gh api "repos/$repo/commits/$sha" --silent 2>/dev/null && break
      sleep 5
    done
    gh api -X POST "repos/$repo/statuses/$sha" --silent \
      -f state=success -f context=local-ci -f description="$desc" \
      && echo "$(date -Is) local-ci: $sha" \
      || echo "$(date -Is) local-ci: posting to $sha FAILED"
  done
  exit 0
fi

base="${1:-}"
if [ -z "$base" ]; then
  git fetch --quiet origin main || echo "ci-local: fetch failed; planning against the local origin/main" >&2
  base="$(git merge-base origin/main HEAD)"
fi
echo "ci-local: $(git rev-parse --short HEAD) against $(git rev-parse --short "$base")"

just ci-fmt
just ci-changes "$base"

# Rail evidence for the base, if `just rail-evidence` recorded it on this
# machine: without it a changed file outside a crate's sources (docs,
# fixtures) widens to the whole workspace. Evidence binds to the cargo
# environment, so it's recorded here, not borrowed from CI.
ev=()
evf="$(git rev-parse --git-common-dir)/rail-evidence/$(git rev-parse "$base").json"
if [ -f "$evf" ]; then
  ev=(--evidence "$evf")
  echo "ci-local: using rail evidence for the base"
fi

# Fail open, like CI: no honest plan means everything.
plan="$(mktemp)"
trap 'rm -f "$plan"' EXIT
gates=""
if cargo rail plan --from "$base" --to HEAD "${ev[@]}" --json >"$plan"; then
  gates="$(python3 scripts/ci-rail-plan.py "$plan")" || gates=""
fi
[ -n "$gates" ] || { echo "ci-local: no plan; running everything"; gates=$'cargo=true\nclippy=true\ntests=true\ntest_filter=\nwasm=true'; }
gate() { sed -n "s/^$1=//p" <<<"$gates" | head -1; }
echo "ci-local: clippy=$(gate clippy) tests=$(gate tests) wasm=$(gate wasm) filter=$(gate test_filter)"

if [ "$(gate tests)" = true ]; then
  TASK_CI_TEST_FILTER="$(gate test_filter)" just ci-tests
fi
if [ "$(gate cargo)" = true ]; then
  just ci-manifests
fi
# rail decides WHETHER clippy runs; it always runs --workspace (a `-p`
# subset changes feature unification and leaves a second artifact set),
# and on a warm tree that re-lints only what changed anyway.
if [ "$(gate clippy)" = true ]; then
  just ci-clippy
fi
if [ "$(gate wasm)" = true ]; then
  just ci-wasm
fi

# Remember the pass for this exact commit (only if the tree is clean, so
# the pass is the commit's): the pre-push hook then vouches without
# re-running. That matters twice over: `gh stack submit` pushes each layer
# separately, and git holds the connection to GitHub open while the hook
# runs, so a long first run can outlast it — the push fails, and the
# retry finds this and goes through at once.
if [ -z "${1:-}" ] && [ -z "$(git status --porcelain --untracked-files=no)" ]; then
  pass="$(git rev-parse --git-common-dir)/ci-local-pass"
  mkdir -p "$pass"
  date -Is > "$pass/$(git rev-parse HEAD)"
fi
echo "ci-local: green"
