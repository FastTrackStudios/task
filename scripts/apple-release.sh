#!/usr/bin/env bash
# The Apple half of a Task release (task-ios.yml, task-macos.yml): the
# version an upload carries, its build number, and the line it adds to the
# tag's GitHub Release. The fleet spec is the `release-workflow` skill,
# references/ios-testflight.md; where they disagree, the skill wins.
#
#   scripts/apple-release.sh version        the marketing version (CFBundleShortVersionString):
#                                           the workspace version, checked to be one Apple takes
#   scripts/apple-release.sh build-number   the build number (CFBundleVersion) for an upload now
#   scripts/apple-release.sh note APP VER BUILD
#                                           add "APP: VER (build BUILD), <run>" to the
#                                           tag's GitHub Release notes (CI, after an upload)
#
# MARKETING VERSION = the release tag's version = `[workspace.package]
# version` (scripts/release.sh version). Every embedded target (the watch
# companion, the widget extension, the Finder extension) is stamped with
# the same version and build number as the app around it: App Store
# Connect rejects a bundle whose extension or watch app disagrees.
#
# BUILD NUMBER: ONE scheme for every Apple upload of Task, iPhone and Mac:
# the UTC unix time, in seconds, at which the workflow computed it. It is
#   - global and ever-increasing across both platforms and every version,
#     so several uploads a day (and a rebuild of the same tag) just work;
#   - above every build App Store Connect already holds: every Task upload
#     so far used `date +%s` (iOS and macOS alike), so a smaller counter
#     (a run number, `1048`) would sort BELOW them, and App Store Connect
#     rejects a Mac build whose CFBundleVersion does not exceed the last
#     upload's (ITMS-90061);
#   - unique without shared state: there is one airlock runner and Apple
#     builds on it never start in the same second.
# The fleet spec's "CI counter" is this clock. It fits in 32 bits until 2106.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

die() { echo "apple-release: $*" >&2; exit 1; }

# Digits and dots only (Apple), three fields, no zero padding (Cargo):
# 26.10.0 yes; 26.01.0, 26.10.0-rc.1, 26.10 no.
is_apple_version() { [[ "$1" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; }

# Every Task upload before this script used unix seconds, so a build
# number at or below this floor (2026-01-01T00:00:00Z) can only be a
# mistake: a clock reset, or a counter from some other scheme.
BUILD_FLOOR=1767225600

cmd_version() {
    local v
    v="$(bash scripts/release.sh version)"
    is_apple_version "$v" || die "workspace version '$v' is not an Apple marketing version (X.Y.Z, digits only, no leading zeros)"
    case "${GITHUB_REF:-}" in
        refs/tags/*) [ "${GITHUB_REF#refs/tags/}" = "v$v" ] || die "tag ${GITHUB_REF#refs/tags/} but the workspace version is $v" ;;
    esac
    echo "$v"
}

cmd_build_number() {
    local n
    n="$(date -u +%s)"
    [ "$n" -gt "$BUILD_FLOOR" ] || die "clock says $n, below the last builds App Store Connect holds: fix the clock"
    echo "$n"
}

# Called by the build scripts on an explicit BUILD_NO override.
cmd_check_build() {
    local n="${1:?check-build needs a number}"
    [[ "$n" =~ ^[1-9][0-9]*$ ]] || die "BUILD_NO '$n' is not a positive integer"
    [ "$n" -gt "$BUILD_FLOOR" ] || die "BUILD_NO $n is below the unix-time builds already in App Store Connect; use scripts/apple-release.sh build-number"
}

# One line per app per upload in the tag's Release. The Release is made
# by deploy.yml (release.sh publish) after the server is live, which can
# be after an upload finishes, so wait for it. task-ios and task-macos
# edit it concurrently, and `gh release edit` replaces the whole body:
# write, wait, re-read, and write again if the line was lost to the other.
cmd_note() {
    local app="${1:?note APP VERSION BUILD}" ver="${2:?}" build="${3:?}"
    local tag repo run line body tries waited=0 wait="${NOTE_WAIT:-2700}"
    case "${GITHUB_REF:-}" in
        refs/tags/v*) tag="${GITHUB_REF#refs/tags/}" ;;
        *) die "note runs on a v* tag (ref is '${GITHUB_REF:-none}')" ;;
    esac
    repo="${GITHUB_REPOSITORY:-FastTrackStudios/task}"
    run="${GITHUB_SERVER_URL:-https://github.com}/$repo/actions/runs/${GITHUB_RUN_ID:-0}"
    line="- $app: $ver (build $build), $run"

    until gh release view "$tag" --repo "$repo" >/dev/null 2>&1; do
        if [ "$waited" -ge "$wait" ]; then
            echo "::warning::no GitHub Release $tag after ${wait}s; add this line to its notes by hand: $line"
            [ -z "${GITHUB_STEP_SUMMARY:-}" ] || echo "Release $tag missing; its line: \`$line\`" >> "$GITHUB_STEP_SUMMARY"
            return 0
        fi
        sleep 60; waited=$((waited + 60))
    done

    for tries in 1 2 3 4 5 6; do
        body="$(gh release view "$tag" --repo "$repo" --json body -q .body)"
        if grep -qxF -- "$line" <<<"$body"; then
            echo "apple-release: $tag notes carry: $line"
            return 0
        fi
        grep -qx '### Apple builds' <<<"$body" || body+=$'\n\n### Apple builds'
        printf '%s\n%s\n' "$body" "$line" > "${RUNNER_TEMP:-/tmp}/apple-note.md"
        gh release edit "$tag" --repo "$repo" --notes-file "${RUNNER_TEMP:-/tmp}/apple-note.md" >/dev/null
        # Let a concurrent writer land before checking we survived it.
        sleep $((${NOTE_SETTLE:-5} + RANDOM % 10 + tries))
    done
    body="$(gh release view "$tag" --repo "$repo" --json body -q .body)"
    grep -qxF -- "$line" <<<"$body" || die "could not keep '$line' in the $tag notes"
    echo "apple-release: $tag notes carry: $line"
}

case "${1:-}" in
    version) shift; cmd_version ;;
    build-number) shift; cmd_build_number ;;
    check-build) shift; cmd_check_build "$@" ;;
    note) shift; cmd_note "$@" ;;
    *) echo "usage: scripts/apple-release.sh version | build-number | check-build N | note APP VERSION BUILD" >&2; exit 2 ;;
esac
