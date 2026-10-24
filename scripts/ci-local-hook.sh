#!/usr/bin/env bash
# prek's pre-push hook (`local-ci` in prek.toml): run local CI
# (scripts/ci-local.sh: fmt, change files, and the clippy / tests / wasm
# that `cargo rail plan` says the change affects) on this machine's warm
# target dir, and once the push lands mark the pushed commit with the
# `Local CI` status. That status is not required on main (only `PR
# checks` is): it says the change's clippy and tests passed on a warm
# machine, and the full set runs after every merge (checks.yml header).
#
# The pushed commit is prek's PRE_COMMIT_TO_REF (prek hands a hook only
# the FIRST ref of a push). On a gh stack, the status also goes to every
# open layer the checked-out tree contains (`targets` below), so a stack
# pushed from its top gets Local CI on every PR from one run, however
# its refs were pushed.
#
# It checks the CHECKED-OUT tree, so it vouches only for a pushed commit
# that tree contains: HEAD or an ancestor, i.e. the layers of a stack
# pushed from its top (`gh stack submit` from the top branch). Anything
# else, or a dirty tree, gets no status; push it from its own checkout, or
# run the full set on GitHub instead:
#
#   gh workflow run checks.yml --ref <branch>     (posts Local CI when green)
#
# Skip: `SKIP=local-ci git push …` (prek's switch; NO_LOCAL_CI=1 also
# works). The PR then has no `Local CI` status; it is not required.
#
# Pushes queue on one lock per clone, so parallel agents don't each build
# the workspace at once (the build machine's free-memory watchdog). A long
# run is best done BEFORE pushing (`just ci-local`): git holds the
# connection to GitHub open while this hook runs, and a 15-minute build
# can outlast it. The pass it records makes the push itself instant.
set -euo pipefail

if [ "${NO_LOCAL_CI:-0}" = "1" ]; then
  echo >&2 "local-ci: NO_LOCAL_CI=1, not checked; the PR gets no Local CI status (not required)."
  exit 0
fi

sha="${PRE_COMMIT_TO_REF:-}"
[ -n "$sha" ] || { echo >&2 "local-ci: no pushed commit (PRE_COMMIT_TO_REF); nothing to vouch for"; exit 0; }
sha="$(git rev-parse "$sha")"
head="$(git rev-parse HEAD)"

if ! git merge-base --is-ancestor "$sha" "$head" 2>/dev/null; then
  echo >&2 "local-ci: ${PRE_COMMIT_LOCAL_BRANCH:-$sha} is not in the checked-out tree; no Local CI status for it."
  exit 0
fi
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  echo >&2 "local-ci: uncommitted changes, so a green run would not prove the pushed commit."
  echo >&2 "          Pushing without a Local CI status; commit them and push again to get it."
  exit 0
fi

REPO_ROOT="$(git rev-parse --show-toplevel)"
COMMON_DIR="$(cd "$(git rev-parse --git-common-dir)" && pwd)"
log="$COMMON_DIR/ci-local.log"

# Which commits a green run at HEAD vouches for: the pushed one, plus every
# open layer of the gh stack HEAD belongs to that HEAD contains. One run on
# the top proves the whole stack (its tree holds every layer's change), so
# each layer's PR gets Local CI however the push named its refs: prek
# passes a hook only the first ref of a push, and `gh stack submit` pushes
# layers one at a time. A layer pushed a moment later is still posted: the
# poster waits for GitHub to have each commit.
targets() {
  echo "$sha"
  command -v gh >/dev/null 2>&1 || return 0
  gh stack view --json 2>/dev/null | python3 -c '
import json, sys
try:
    stack = json.load(sys.stdin)
except Exception:
    sys.exit(0)
for b in stack.get("branches") or []:
    if not b.get("isMerged") and b.get("head"):
        print(b["head"])
' | while read -r h; do
    git merge-base --is-ancestor "$h" "$head" 2>/dev/null && echo "$h"
  done
}

post() {
  mapfile -t shas < <(targets | awk '!seen[$0]++')
  nohup bash "$REPO_ROOT/scripts/ci-local.sh" --post "${shas[@]}" >>"$log" 2>&1 </dev/null &
  echo >&2 "local-ci: green; posting to ${#shas[@]} commit(s) once GitHub has them (log: $log)"
}

# Already green at this exact HEAD (`just ci-local`, or the push of a
# lower stack layer a moment ago): vouch without re-running.
if [ -f "$COMMON_DIR/ci-local-pass/$head" ]; then
  echo >&2 "local-ci: already passed at ${head:0:8}"
  post
  exit 0
fi

if command -v flock >/dev/null 2>&1; then
  exec 9>"$COMMON_DIR/ci-local.lock"
  flock -n 9 || { echo >&2 "local-ci: another push's local CI is running; waiting…"; flock 9; }
  # The push we waited on may have just passed this same commit.
  if [ -f "$COMMON_DIR/ci-local-pass/$head" ]; then
    post
    exit 0
  fi
fi

# ci-local needs the dev shell (cargo-rail, nextest, the desktop stack's
# system libs). If `pkg-config` already sees pango + libsoup we are in
# this repo's shell; otherwise re-enter it, even when IN_NIX_SHELL is set
# (that may be some other project's shell).
if { command -v pkg-config >/dev/null 2>&1 && pkg-config --exists pango libsoup-3.0 2>/dev/null; } \
  || ! command -v nix >/dev/null 2>&1; then
  bash "$REPO_ROOT/scripts/ci-local.sh"
else
  nix develop "$REPO_ROOT" -c bash "$REPO_ROOT/scripts/ci-local.sh"
fi

post
