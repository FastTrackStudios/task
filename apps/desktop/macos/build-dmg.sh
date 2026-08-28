#!/usr/bin/env bash
# Build the Task macOS app as a **Developer-ID .dmg** for direct download:
# hardened runtime, notarized, stapled, with the sync agent inside the
# bundle so installing the app installs background file sync.
#
# The sibling script (deploy-testflight-macos.sh) targets TestFlight / the
# Mac App Store instead. The difference that matters here is not the
# distribution channel but the sandbox: an App-Sandboxed build may not write
# into ~/Library/LaunchAgents, so it cannot register the sync agent that
# keeps files moving when the app is closed. This build can, which is why
# it is the one to ship for syncing between your own machines.
#
#   KEYCHAIN=fts-build.keychain KEYCHAIN_PW=fts-build \
#     bash apps/desktop/macos/build-dmg.sh
#
# Env knobs (all optional):
#   MARKETING_VER   CFBundleShortVersionString (default 0.0.2)
#   BUILD_NO        CFBundleVersion (default: unix time)
#   SKIP_BUILD=1    reuse the existing .app (iterate on sign/dmg)
#   DRY_RUN=1       stop after the signed .dmg — no notarization, no upload
#   OUT_DIR         where the .dmg lands (default: target/dmg)
#
# Needs ~/.appstoreconnect/config.env (the same file the TestFlight flow
# reads) for the notarization credentials, and a "Developer ID Application"
# identity in the keychain. `setup-macos.sh` provisions the App Store
# identities; a Developer-ID certificate is a separate one-time download
# from developer.apple.com — this script says so plainly rather than
# pretending it can mint one.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"     # apps/desktop/macos
ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"      # repo root

DX_PACKAGE="${DX_PACKAGE:-task-app-desktop}"
DX_APP_DIR="${DX_APP_DIR:-apps/desktop}"
DX_BUNDLE_ID="${DX_BUNDLE_ID:-app.fasttrackstudio.task}"
PRODUCT_NAME="${PRODUCT_NAME:-Task}"
DX_TAILWIND="${DX_TAILWIND-../tailwind.css}"
ICON_1024="${ICON_1024:-$ROOT/apps/mobile/ios/Assets.xcassets/AppIcon.appiconset/icon-1024.png}"
OUT_DIR="${OUT_DIR:-$ROOT/target/dmg}"
MARKETING_VER="${MARKETING_VER:-0.0.2}"
BUILD_NO="${BUILD_NO:-$(date +%s)}"

# Notarization credentials live where the TestFlight flow already keeps
# them. Absent is fine until we actually notarize — DRY_RUN never does.
# shellcheck disable=SC1090
[ -f "$HOME/.appstoreconnect/config.env" ] && source "$HOME/.appstoreconnect/config.env"

NIX="${NIX:-}"
if [ -z "$NIX" ]; then
    for c in /run/current-system/sw/bin/nix /nix/var/nix/profiles/default/bin/nix "$(command -v nix 2>/dev/null || true)"; do
        [ -n "$c" ] && [ -x "$c" ] && { NIX="$c"; break; }
    done
fi

KEYCHAIN="${KEYCHAIN:-login.keychain-db}"
KEYCHAIN_PW="${KEYCHAIN_PW:-}"
[ -n "$KEYCHAIN_PW" ] && security unlock-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN"

# ── The Developer-ID identity ───────────────────────────────────────────────
# Not the same certificate as "Apple Distribution", and not obtainable from
# the ASC API the way the App Store ones are: Developer ID certificates are
# issued to the Account Holder through the developer portal. So this looks
# it up and stops with an instruction rather than a stack trace.
SIGN_ID="${SIGN_ID:-$(security find-identity -v -p codesigning "$KEYCHAIN" \
    | awk -F'"' '/Developer ID Application/ {print $2; exit}')}"
if [ -z "$SIGN_ID" ]; then
    echo "ERROR: no \"Developer ID Application\" identity in $KEYCHAIN." >&2
    echo "       Create one at developer.apple.com → Certificates → Developer ID Application," >&2
    echo "       download it, and import it:  security import <cert.p12> -k $KEYCHAIN" >&2
    echo "       (The App Store identities setup-macos.sh provisions are a different kind" >&2
    echo "        and cannot sign a directly-distributed .dmg.)" >&2
    exit 1
fi
echo "=== signing identity: $SIGN_ID ==="

# ── Build ───────────────────────────────────────────────────────────────────
APP_GLOB="$ROOT/target/dx/$DX_PACKAGE/release/macos/*.app"
if [ "${SKIP_BUILD:-}" = "1" ]; then
    echo "=== SKIP_BUILD=1 — reusing the existing .app ==="
else
    echo "=== building macOS app (dx, release) ==="
    # shellcheck disable=SC2086
    rm -rf $APP_GLOB
    "$NIX" develop "$ROOT" --accept-flake-config -c bash -c "
        set -euo pipefail
        cd '$ROOT/$DX_APP_DIR'
        ${DX_TAILWIND:+(cd \"\$(dirname '$DX_TAILWIND')\" && tailwindcss -i \"\$(basename '$DX_TAILWIND')\" -o '$ROOT/$DX_APP_DIR/assets/tailwind.css')}
        dx build --platform macos --release
    " > /tmp/task-macos-dmg-build.log 2>&1 || true
    tail -3 /tmp/task-macos-dmg-build.log
fi
# shellcheck disable=SC2086
APP="$(ls -d $APP_GLOB 2>/dev/null | head -1)"
[ -n "$APP" ] && [ -d "$APP" ] || { echo "ERROR: build produced no app"; tail -30 /tmp/task-macos-dmg-build.log; exit 1; }
PLIST="$APP/Contents/Info.plist"
echo "=== app bundle: $APP ==="

# ── The sync agent, inside the app ──────────────────────────────────────────
# This is what makes the DMG worth having over the App Store build: the app
# registers this binary as a LaunchAgent on first run, and it keeps syncing
# after the window closes. Contents/MacOS because that is where the app looks
# (next to its own executable) and where nested code may be signed.
echo "=== building the sync agent ==="
"$NIX" develop "$ROOT" --accept-flake-config -c bash -c "
    set -euo pipefail
    cd '$ROOT'
    cargo build --release -p files-daemon --features daemon-bin --bin fts-files-daemon
"
cp "$ROOT/target/release/fts-files-daemon" "$APP/Contents/MacOS/fts-files-daemon"

# ── Info.plist ──────────────────────────────────────────────────────────────
pb() { /usr/libexec/PlistBuddy -c "Set :$1 $2" "$PLIST" 2>/dev/null \
      || /usr/libexec/PlistBuddy -c "Add :$1 string $2" "$PLIST"; }
pb CFBundleIdentifier "$DX_BUNDLE_ID"
pb CFBundleName "$PRODUCT_NAME"
pb CFBundleDisplayName "$PRODUCT_NAME"
pb CFBundleShortVersionString "$MARKETING_VER"
pb CFBundleVersion "$BUILD_NO"
pb CFBundlePackageType APPL
pb LSMinimumSystemVersion "12.0"
pb LSApplicationCategoryType "public.app-category.productivity"

# ── Icon ────────────────────────────────────────────────────────────────────
if [ -f "$ICON_1024" ]; then
    ICONSET="$(mktemp -d)/AppIcon.iconset"
    mkdir -p "$ICONSET"
    for s in 16 32 128 256 512; do
        sips -z "$s" "$s" "$ICON_1024" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
        d=$((s * 2))
        sips -z "$d" "$d" "$ICON_1024" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
    done
    mkdir -p "$APP/Contents/Resources"
    iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
    pb CFBundleIconFile AppIcon
fi
echo "=== app: $PRODUCT_NAME ($DX_BUNDLE_ID) $MARKETING_VER build $BUILD_NO ==="

# ── Sign, inside out ────────────────────────────────────────────────────────
# Hardened runtime (--options runtime) is required for notarization. Nested
# code first: signing anything inside the bundle after the bundle itself
# invalidates the outer signature.
echo "=== signing (Developer ID + hardened runtime) ==="
ENTITLEMENTS="$SCRIPT_DIR/Task-devid.entitlements"
find "$APP" \( -name "*.dylib" -o -name "*.so" -o -name "*.framework" \) -print0 \
    | while IFS= read -r -d '' f; do
        codesign --force --keychain "$KEYCHAIN" --timestamp --options runtime \
            --sign "$SIGN_ID" "$f"
      done
# The agent is a program of its own, and gets its own hardened-runtime
# signature with the same entitlements — it is launched by launchd directly,
# not through the app, so it is verified on its own terms.
codesign --force --keychain "$KEYCHAIN" --timestamp --options runtime \
    --entitlements "$ENTITLEMENTS" --sign "$SIGN_ID" "$APP/Contents/MacOS/fts-files-daemon"
codesign --force --keychain "$KEYCHAIN" --timestamp --options runtime \
    --entitlements "$ENTITLEMENTS" --sign "$SIGN_ID" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# ── The disk image ──────────────────────────────────────────────────────────
# A staging directory with the app and a symlink to /Applications: the
# drag-to-install window everyone recognises. `hdiutil` builds it read-only
# and compressed (UDZO).
echo "=== building the disk image ==="
mkdir -p "$OUT_DIR"
DMG="$OUT_DIR/$PRODUCT_NAME-$MARKETING_VER-$BUILD_NO.dmg"
STAGE="$(mktemp -d)/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "$PRODUCT_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG"

# The disk image is signed too: Gatekeeper checks the .dmg a person
# downloaded, not only the .app they drag out of it.
codesign --force --keychain "$KEYCHAIN" --timestamp --sign "$SIGN_ID" "$DMG"

if [ "${DRY_RUN:-}" = "1" ]; then
    echo "=== DRY_RUN=1 — signed, not notarized. Unnotarized: $DMG ==="
    echo "    Gatekeeper will refuse this on any machine but this one."
    exit 0
fi

# ── Notarize + staple ───────────────────────────────────────────────────────
# Apple must see the build before another Mac will run it. `notarytool
# submit --wait` blocks until the verdict; a rejection prints the log URL,
# which is the only useful thing to look at when it happens.
: "${ASC_KEY_ID:?ASC_KEY_ID missing — needed to notarize (see ~/.appstoreconnect/config.env)}"
: "${ASC_ISSUER_ID:?ASC_ISSUER_ID missing — needed to notarize}"
: "${ASC_KEY_PATH:?ASC_KEY_PATH missing — needed to notarize}"

echo "=== notarizing (this takes a few minutes) ==="
xcrun notarytool submit "$DMG" \
    --key "$ASC_KEY_PATH" --key-id "$ASC_KEY_ID" --issuer "$ASC_ISSUER_ID" \
    --wait

# Staple the ticket into the image so it opens on a machine with no
# network — without this a first launch offline is refused.
xcrun stapler staple "$DMG"
xcrun stapler validate "$DMG"

# And the assertion that matters: what Gatekeeper itself says about the app
# a person will drag out. A signed, notarized, stapled bundle answers
# "accepted"; anything else here means the download will be refused.
MOUNT="$(mktemp -d)"
hdiutil attach "$DMG" -nobrowse -readonly -mountpoint "$MOUNT" >/dev/null
spctl --assess --type execute --verbose=4 "$MOUNT/$(basename "$APP")" || {
    echo "ERROR: Gatekeeper refused the app inside the image" >&2
    hdiutil detach "$MOUNT" >/dev/null || true
    exit 1
}
hdiutil detach "$MOUNT" >/dev/null

echo
echo "=== done ==="
echo "dmg: $DMG"
echo "Drag Task to Applications, open it once, sign in — the sync agent"
echo "installs itself and this machine pairs with your org."
