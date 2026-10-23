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

# Fail open, like CI: no honest plan means everything.
plan="$(mktemp)"
trap 'rm -f "$plan"' EXIT
gates=""
if cargo rail plan --from "$base" --to HEAD --json >"$plan"; then
  gates="$(python3 scripts/ci-rail-plan.py "$plan")" || gates=""
fi
[ -n "$gates" ] || { echo "ci-local: no plan; running everything"; gates=$'cargo=true\ntests=true\ntest_filter=\nwasm=true'; }
gate() { sed -n "s/^$1=//p" <<<"$gates" | head -1; }
echo "ci-local: cargo=$(gate cargo) tests=$(gate tests) wasm=$(gate wasm) filter=$(gate test_filter)"

if [ "$(gate tests)" = true ]; then
  TASK_CI_TEST_FILTER="$(gate test_filter)" just ci-tests
fi
if [ "$(gate cargo)" = true ]; then
  just ci-manifests
  just ci-clippy
fi
if [ "$(gate wasm)" = true ]; then
  just ci-wasm
fi
echo "ci-local: green"
