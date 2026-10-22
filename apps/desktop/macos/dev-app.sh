#!/usr/bin/env bash
# Build and run the Task desktop app for development AS "Task" — the name,
# bundle id and icon the release build has — instead of a bare
# `task-app-desktop` executable in the Dock.
#
# `dx serve --platform desktop` runs the binary unbundled, so macOS shows
# the crate name and a generic exec icon. This builds the same debug app
# with `dx build --platform macos`, stamps the bundle with what
# build-dmg.sh stamps (minus signing identity and the sync agent), signs it
# ad hoc, and runs its executable in the foreground so the caller's
# environment (TASK_VOX_URL, TASK_DEMO_CAST, XDG_DATA_HOME, …) reaches it.
#
#   bash apps/desktop/macos/dev-app.sh            # build + run
#   SKIP_BUILD=1 bash apps/desktop/macos/dev-app.sh   # rerun the last build
#
# Run from anywhere; it enters the nix dev shell itself for the build.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"     # apps/desktop/macos
ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"      # repo root
PRODUCT_NAME="${PRODUCT_NAME:-Task}"
BUNDLE_ID="${BUNDLE_ID:-app.fasttrackstudio.task}"
ICON_1024="${ICON_1024:-$ROOT/apps/mobile/ios/Assets.xcassets/AppIcon.appiconset/icon-1024.png}"
APP_GLOB="$ROOT/target/dx/task-app-desktop/debug/macos/*.app"
ICON_CACHE="$ROOT/target/dev-app/AppIcon.icns"

NIX="${NIX:-$(command -v nix 2>/dev/null || echo /nix/var/nix/profiles/default/bin/nix)}"

if [ -z "${SKIP_BUILD:-}" ]; then
    # The stylesheet is generated (and git-ignored): rebuild it with the app.
    "$NIX" develop "$ROOT" -c bash -c "
        set -euo pipefail
        unset RUSTC_WRAPPER
        bash '$ROOT/scripts/architect-ui-link.sh' >/dev/null
        cd '$ROOT/apps' && tailwindcss -i tailwind.css -o desktop/assets/tailwind.css >/dev/null
        cd '$ROOT/apps/desktop' && dx build --platform macos
    "
fi

# shellcheck disable=SC2086
APP="$(ls -d $APP_GLOB 2>/dev/null | head -1)"
[ -n "$APP" ] && [ -d "$APP" ] || { echo "dev-app: the build produced no .app under $APP_GLOB" >&2; exit 1; }
PLIST="$APP/Contents/Info.plist"

pb() { /usr/libexec/PlistBuddy -c "Set :$1 $2" "$PLIST" 2>/dev/null \
      || /usr/libexec/PlistBuddy -c "Add :$1 string $2" "$PLIST"; }
pb CFBundleIdentifier "$BUNDLE_ID"
pb CFBundleName "$PRODUCT_NAME"
pb CFBundleDisplayName "$PRODUCT_NAME"
pb CFBundlePackageType APPL

# The icon, made once and reused (sips + iconutil take a few seconds).
if [ -f "$ICON_1024" ]; then
    if [ ! -f "$ICON_CACHE" ] || [ "$ICON_1024" -nt "$ICON_CACHE" ]; then
        ICONSET="$(mktemp -d)/AppIcon.iconset"
        mkdir -p "$ICONSET" "$(dirname "$ICON_CACHE")"
        for s in 16 32 128 256 512; do
            sips -z "$s" "$s" "$ICON_1024" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
            d=$((s * 2))
            sips -z "$d" "$d" "$ICON_1024" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
        done
        iconutil -c icns "$ICONSET" -o "$ICON_CACHE"
    fi
    mkdir -p "$APP/Contents/Resources"
    cp "$ICON_CACHE" "$APP/Contents/Resources/AppIcon.icns"
    pb CFBundleIconFile AppIcon
fi

# Ad-hoc signature after the plist edits, or macOS refuses the changed
# bundle. Launch Services caches names/icons per bundle path: re-register.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP" >/dev/null 2>&1 || true

EXE="$APP/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$PLIST")"
echo ">> $PRODUCT_NAME ($BUNDLE_ID): $APP"
exec "$EXE"
