#!/usr/bin/env bash
# Local CI: the expensive half of the PR gate, run on this machine's warm
# target dir instead of a billed runner.
#
#   scripts/ci-local.sh [BASE]        run the checks (`just ci-local`)
#   scripts/ci-local.sh --post SHA…   mark each commit `local-ci` on GitHub
#                                     once GitHub has it (the pre-push hook
#                                     runs this detached, after the push)
#
# PRs into main run only a minimal free hosted job (checks.yml: fmt, change
# files, rail route cases) plus the `local-ci` status this posts. The rest
# of the PR gate runs HERE: the static guards (one version everywhere,
# manifests + lockfile, the architect-ui sheet), then clippy, the tests and
# the wasm check for what the change affects: `cargo rail plan` from BASE (default: the merge-base with
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
  # Every layer's push re-posts the whole stack; post each commit once.
  posted="$(git rev-parse --git-common-dir)/ci-local-posted"
  mkdir -p "$posted"
  for sha in "$@"; do
    [ -f "$posted/$sha" ] && continue
    # The hook runs before the push lands: wait for GitHub to know the
    # commit (a status on an unknown sha is a 422).
    for _ in $(seq 1 60); do
      gh api "repos/$repo/commits/$sha" --silent 2>/dev/null && break
      sleep 5
    done
    if gh api -X POST "repos/$repo/statuses/$sha" --silent \
      -f state=success -f context=local-ci -f description="$desc"; then
      date -Is > "$posted/$sha"
      echo "$(date -Is) local-ci: $sha"
    else
      echo "$(date -Is) local-ci: posting to $sha FAILED"
    fi
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
# Static guards, seconds each: every artifact reports the release version,
# Cargo.lock matches the manifests, the token sheet comes through the
# architect-ui link (STA-392).
just ci-versions
just ci-manifests
just ci-architect-css

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
echo "ci-local: clippy=$(gate clippy) tests=$(gate tests) wasm=$(gate wasm) ($(gate wasm_packages)) filter=$(gate test_filter)"

if [ "$(gate tests)" = true ]; then
  TASK_CI_TEST_FILTER="$(gate test_filter)" just ci-tests
fi
# rail decides WHETHER clippy runs; it always runs --workspace (a `-p`
# subset changes feature unification and leaves a second artifact set),
# and on a warm tree that re-lints only what changed anyway.
if [ "$(gate clippy)" = true ]; then
  just ci-clippy
fi
if [ "$(gate wasm)" = true ]; then
  # shellcheck disable=SC2046 # a space-separated package list, split on purpose
  just ci-wasm $(gate wasm_packages)
fi

# Remember the pass for this exact commit (only if the tree is clean, so
# the pass is the commit's): the pre-push hook then vouches without
# re-running. That matters twice over: `gh stack submit` pushes each layer
# separately, and git holds the connection to GitHub open while the hook
# runs, so a long first run can outlast it — the push fails, and the
# retry finds this and goes through at once.
dirty="$(git status --porcelain --untracked-files=no)"
if [ -n "${1:-}" ]; then
  echo "ci-local: BASE given, so no pass recorded (a narrower run doesn't vouch for the commit)"
elif [ -n "$dirty" ]; then
  echo "ci-local: the tree changed, so no pass recorded for $(git rev-parse --short HEAD):"
  echo "$dirty"
else
  pass="$(git rev-parse --git-common-dir)/ci-local-pass"
  mkdir -p "$pass"
  date -Is > "$pass/$(git rev-parse HEAD)"
  echo "ci-local: pass recorded for $(git rev-parse --short HEAD)"
fi
echo "ci-local: green"
