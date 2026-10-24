#!/usr/bin/env bash
# prek's post-merge hook (`rail-evidence` in prek.toml): after a pull lands
# on main, record rail's evidence for the new HEAD (`just rail-evidence`)
# in the background, so the next `ci-local` from a branch off this commit
# narrows a docs or fixture change instead of running the whole workspace.
#
# Local, not CI's: evidence binds to the cargo environment (CARGO_TARGET_DIR,
# CARGO_INCREMENTAL and the rustflags, exactly as set), and CI's differs from
# this machine's, so evidence recorded on GitHub never matches a local plan.
#
# The recording is one test build + one clippy on the warm target dir, so it
# runs detached and under local CI's lock (one build per clone at a time);
# the pull returns at once. Output: <git-dir>/rail-evidence/hook.log.
#
# Skip: `NO_RAIL_EVIDENCE=1 git pull`, or `SKIP=rail-evidence`.
set -euo pipefail

[ "${NO_RAIL_EVIDENCE:-0}" = "1" ] && exit 0
[ "$(git rev-parse --abbrev-ref HEAD)" = "main" ] || exit 0

common="$(git rev-parse --git-common-dir)"
head="$(git rev-parse HEAD)"
dir="$common/rail-evidence"
mkdir -p "$dir"
[ -f "$dir/$head.json" ] && exit 0
# rail records nothing when tracked files differ from HEAD.
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  echo >&2 "rail-evidence: uncommitted changes on main; not recording (run \`just rail-evidence\` once clean)."
  exit 0
fi

# Keep the newest 20 commits' evidence.
find "$dir" -maxdepth 1 -name '*.json' -printf '%T@ %p\n' | sort -rn | tail -n +21 | cut -d' ' -f2- | xargs -r -d '\n' rm -f

echo >&2 "rail-evidence: recording for ${head:0:12} in the background ($dir/hook.log)"
# shellcheck disable=SC2016 # the script reads its values as $1/$2, below
nohup setsid bash -c '
  exec 9>"$1/ci-local.lock"
  flock 9
  # main may have moved on while we waited for the lock.
  [ "$(git rev-parse HEAD)" = "$2" ] || { echo "HEAD moved past $2; skipped"; exit 0; }
  echo "$(date -Is) recording $2"
  just rail-evidence && echo "$(date -Is) done $2"
' rail-evidence "$common" "$head" >>"$dir/hook.log" 2>&1 </dev/null &
exit 0
