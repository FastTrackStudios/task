#!/usr/bin/env bash
# Build the Task iPhone app for the simulator, install it, and launch it
# against the local demo server — the phone's half of the dev loop
# (`just demo serve` is the other half).
#
#   just mobile-sim                 # build + run on the booted simulator
#   apps/mobile/ios/build-ios.sh [--sim <udid|booted>] [--no-build]
#
# Run from anywhere; it enters the nix dev shell itself. The env dance is
# REQUIRED (same as deploy-testflight.sh): nixpkgs ships a fake xcbuild
# `xcrun` and its SDK env hides Xcode's, so iOS cross-compiles need the
# real xcrun first on PATH and the nix SDK vars unset.
#
# The app it launches dials TASK_VOX_URL (default: the demo's ACME server
# on :18080) and, being a debug build, signs in as the demo cast
# (TASK_DEMO_CAST, as `just demo web` bakes it). The simulator shares the
# Mac's loopback, so 127.0.0.1 is the Mac. Both reach the app through
# `simctl launch`'s SIMCTL_CHILD_ environment — the app has no shell.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
SIM="booted"
BUILD=1
LOCAL=0
PROFILE=debug
while [ $# -gt 0 ]; do
    case "$1" in
        --sim) SIM="$2"; shift 2 ;;
        --no-build) BUILD=0; shift ;;
        # No host at all: the phone runs on its own in-process node,
        # signed in as the device's person, the example studio planted
        # on first run (a debug build's `TASK_NODE_EXAMPLE` default).
        --local) LOCAL=1; shift ;;
        # The profile TestFlight ships, for a crash only release shows.
        --release) PROFILE=release; shift ;;
        *) echo "usage: $0 [--sim <udid|booted>] [--no-build] [--local]" >&2; exit 2 ;;
    esac
done

if [ "$BUILD" = 1 ]; then
    BIN_IOS="${TMPDIR:-/tmp}/task-bin-ios"
    mkdir -p "$BIN_IOS"
    ln -sf /usr/bin/xcrun "$BIN_IOS/xcrun"
    # Tailwind v4's content detection is rooted at the working directory,
    # so the sheet is built from the input's own directory (apps/), as
    # apps/tailwind_build.rs and deploy-testflight.sh do.
    nix develop "$ROOT" -c bash -c "
        set -euo pipefail
        unset DEVELOPER_DIR SDKROOT RUSTC_WRAPPER
        export PATH='$BIN_IOS':\$PATH
        cd '$ROOT/apps' && tailwindcss -i tailwind.css -o mobile/assets/tailwind.css
        cd '$ROOT/apps/mobile' && dx build --platform ios $( [ "$PROFILE" = release ] && echo --release )"
fi

# dx names the bundle from the package (TaskAppMobile.app today; it has
# changed between dx versions), so take the newest one it wrote.
APP="$(ls -dt "$ROOT"/target/dx/task-app-mobile/$PROFILE/ios/*.app 2>/dev/null | head -1)"
[ -n "$APP" ] && [ -f "$APP/Info.plist" ] || { echo "ERROR: dx produced no app" >&2; exit 1; }

# Info.plist — the same keys the TestFlight build writes.
# shellcheck source=app-plist.sh
source "$HERE/app-plist.sh"
task_app_plist "$APP"

# The asset catalog (icon + the launch screen's LaunchBackground colour),
# compiled for the simulator. Best effort: a dev build without it still
# runs, on the system's default launch colour.
if xcrun actool "$HERE/Assets.xcassets" --compile "$APP" --platform iphonesimulator \
    --minimum-deployment-target 15.0 --app-icon AppIcon \
    --output-partial-info-plist "${TMPDIR:-/tmp}/task-sim-icon.plist" >/dev/null 2>&1; then
    /usr/libexec/PlistBuddy -c "Merge ${TMPDIR:-/tmp}/task-sim-icon.plist" "$APP/Info.plist" 2>/dev/null || true
else
    echo "note: actool failed; launching without the compiled asset catalog" >&2
fi

# Editing Info.plist broke dx's signature; the simulator takes ad hoc.
codesign --force --sign - "$APP" >/dev/null 2>&1
echo "built: $APP"

BUNDLE="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Info.plist")"
xcrun simctl install "$SIM" "$APP"

# The demo cast, built the way scripts/demo.sh builds it — from the seed's
# own password, so this never drifts from what `just demo serve` planted.
PW="$(sed -n 's/^pub const PASSWORD: &str = "\(.*\)";$/\1/p' "$ROOT/apps/server/src/example_org.rs")"
CAST="alice@acme.test:$PW:Alice:alice,sam@acme.test:$PW:Sam:sam,casey@client.test:$PW:Casey:casey,riley@rockstars.test:$PW:Riley:riley"

if [ "$LOCAL" = 1 ]; then
    # Nothing to dial and nobody to sign in as but the node's own person.
    SIMCTL_CHILD_RUST_BACKTRACE=1 \
        xcrun simctl launch --terminate-running-process "$SIM" "$BUNDLE" >/dev/null
    echo "launched $BUNDLE on simulator $SIM — on its own node, no server"
    exit 0
fi
SIMCTL_CHILD_TASK_VOX_URL="${TASK_VOX_URL:-ws://127.0.0.1:18080/vox}" \
SIMCTL_CHILD_TASK_DEMO_CAST="${TASK_DEMO_CAST:-$CAST}" \
SIMCTL_CHILD_RUST_BACKTRACE=1 \
    xcrun simctl launch --terminate-running-process "$SIM" "$BUNDLE" >/dev/null
echo "launched $BUNDLE on simulator $SIM"
