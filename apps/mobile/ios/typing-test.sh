#!/usr/bin/env bash
# The typing test: every markdown construct, typed into notes in the Task
# app's own note view on the iOS Simulator, through the iOS keyboard's
# input calls on the app's web view (see crates/ui/src/typing_test.rs) —
# then checked for text, caret and rendering, with a screenshot of each.
#
#   just ios-typing                              # build, install, run
#   apps/mobile/ios/typing-test.sh [--no-build] [--sim <udid|booted>]
#
# Screenshots and the report land in target/typing-test/. Exits non-zero
# when any case fails.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
SIM="booted"
BUILD=1
while [ $# -gt 0 ]; do
    case "$1" in
        --sim) SIM="$2"; shift 2 ;;
        --no-build) BUILD=0; shift ;;
        *) echo "usage: $0 [--no-build] [--sim <udid|booted>]" >&2; exit 2 ;;
    esac
done

BUNDLE=app.fasttrackstudio.task
OUT="$ROOT/target/typing-test"

if [ "$BUILD" = 1 ]; then
    bash "$HERE/build-ios.sh" --local --sim "$SIM"
fi

xcrun simctl terminate "$SIM" "$BUNDLE" 2>/dev/null || true
DATA="$(xcrun simctl get_app_container "$SIM" "$BUNDLE" data)"
REPORT_DIR="$DATA/Documents/typing-test"
rm -rf "$REPORT_DIR" "$OUT"
mkdir -p "$OUT"

# The app has no shell: the flag reaches it through simctl's
# SIMCTL_CHILD_ environment, as build-ios.sh's do.
SIMCTL_CHILD_TASK_TYPING_TEST=1 xcrun simctl launch "$SIM" "$BUNDLE" >/dev/null
echo "typing test: running in the app on $SIM …"

# Follow the app's progress; screenshot each case as it finishes.
last=-1
deadline=$((SECONDS + 900))
while [ $SECONDS -lt $deadline ]; do
    if [ -f "$REPORT_DIR/progress.json" ]; then
        done_flag="$(jq -r '.done' "$REPORT_DIR/progress.json" 2>/dev/null || echo false)"
        case_ix="$(jq -r '.case' "$REPORT_DIR/progress.json" 2>/dev/null || echo -1)"
        if [ "$done_flag" = true ]; then break; fi
        if [ "$case_ix" != "$last" ] && [ "$case_ix" -ge 0 ]; then
            last="$case_ix"
            name="$(jq -r '.name' "$REPORT_DIR/progress.json")"
            slug="$(printf '%02d-%s' "$((case_ix + 1))" "$name" | tr -cs 'A-Za-z0-9-' '-' | tr 'A-Z' 'a-z')"
            # The simulator service may not write to every volume: shoot
            # into a temp dir, then copy.
            shot="$(mktemp -d)"
            if xcrun simctl io "$SIM" screenshot "$shot/s.png" >/dev/null 2>&1; then
                cp "$shot/s.png" "$OUT/${slug%-}.png"
            fi
            rm -rf "$shot"
        fi
    fi
    sleep 0.3
done

if [ ! -f "$REPORT_DIR/report.json" ]; then
    echo "typing test: no report (timed out, or the app never ran it)" >&2
    exit 1
fi
cp "$REPORT_DIR/report.json" "$OUT/report.json"

echo "typed through: $(jq -r '.[0].via // "?"' "$OUT/report.json")"
jq -r '.[] | (if .passed then "✓ " else "✗ " end) + .name, (.failures[] | "    " + .)' "$OUT/report.json"
total="$(jq 'length' "$OUT/report.json")"
passed="$(jq '[.[] | select(.passed)] | length' "$OUT/report.json")"
echo
echo "$passed/$total passed — screenshots in $OUT"
[ "$passed" = "$total" ]
