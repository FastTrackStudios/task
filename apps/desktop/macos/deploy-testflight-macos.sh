#!/usr/bin/env bash
# Build the Task macOS desktop app (dx/Dioxus), App-Sandbox it, wrap it in a
# signed installer .pkg, and upload to TestFlight for Mac — the macOS mirror
# of the Task iOS flow (.github/workflows/task-ios.yml →
# apps/mobile/ios/deploy-testflight.sh). TestFlight then keeps the
# app auto-updated on testers' Macs.
#
# Distinct from the FastTrackStudio repo's deploy-macos.sh (which this script
# started from): that one makes a Developer-ID .dmg for direct download
# (hardened runtime + notarization); this one targets TestFlight / the Mac
# App Store — App Sandbox entitlements, "Apple Distribution" on the .app,
# "Mac Installer Distribution" on the .pkg wrapper, App Store Connect upload.
#
# Runs on airlock (headless) with the dedicated keychain:
#   KEYCHAIN=fts-build.keychain KEYCHAIN_PW=fts-build \
#     bash apps/desktop/macos/deploy-testflight-macos.sh
#
# Env knobs (all optional):
#   MARKETING_VER  CFBundleShortVersionString (default 0.0.2) — keep in step
#                  with the iOS build's; both platforms share the app record.
#   BUILD_NO       CFBundleVersion (default: unix time). App Store Connect
#                  tracks build numbers PER PLATFORM, so the macOS train
#                  can't collide with the iOS one; unix time keeps it
#                  monotonic and doubles as the upload timestamp — the exact
#                  scheme the iOS flow uses.
#   SKIP_BUILD=1   reuse the existing .app (iterate on sign/pkg/upload).
#   DRY_RUN=1      stop after producing + verifying the signed .pkg —
#                  everything except the App Store Connect upload.
#
# Needs ~/.appstoreconnect/config.env, and setup-macos.sh run once (it
# provisions both signing identities into the build keychain via the ASC
# API). The provisioning profile is (re)minted every run by
# mint-mas-profile.rb, exactly like the iOS flow re-mints its profile.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"        # apps/desktop/macos
ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"      # repo root

DX_PACKAGE="${DX_PACKAGE:-task-app-desktop}"
DX_APP_DIR="${DX_APP_DIR:-apps/desktop}"
DX_BUNDLE_ID="${DX_BUNDLE_ID:-app.fasttrackstudio.task}"
PRODUCT_NAME="${PRODUCT_NAME:-Task}"
PROFILE_NAME="${PROFILE_NAME:-Task macOS App Store}"
# The three Task apps share ONE tailwind input at apps/tailwind.css
# (each app's assets/tailwind.css is generated output). Relative to DX_APP_DIR;
# no colon so an explicitly-empty DX_TAILWIND is honored.
DX_TAILWIND="${DX_TAILWIND-../tailwind.css}"
# 1024px master the .icns is generated from (same art as the iOS app icon).
ICON_1024="${ICON_1024:-$ROOT/apps/mobile/ios/Assets.xcassets/AppIcon.appiconset/icon-1024.png}"

# shellcheck disable=SC1090
source "$HOME/.appstoreconnect/config.env"

NIX="${NIX:-}"
if [ -z "$NIX" ]; then
    for c in /run/current-system/sw/bin/nix /nix/var/nix/profiles/default/bin/nix "$(command -v nix 2>/dev/null || true)"; do
        [ -n "$c" ] && [ -x "$c" ] && { NIX="$c"; break; }
    done
fi

KEYCHAIN="${KEYCHAIN:-login.keychain-db}"
KEYCHAIN_PW="${KEYCHAIN_PW:-}"
[ -n "$KEYCHAIN_PW" ] && security unlock-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN"

# ── Signing identities (provisioned once by setup-macos.sh) ──────────────────
# The .app is signed with "Apple Distribution" (the same identity the iOS
# flow uses); the installer .pkg wrapper needs its own "Mac Installer
# Distribution" identity ("3rd Party Mac Developer Installer: …" CN). This
# script only LOOKS THEM UP — if either is missing, run setup-macos.sh once
# (it obtains + installs whatever is absent, no Xcode UI needed).
SIGN_ID="$(security find-identity -v -p codesigning "$KEYCHAIN" \
    | awk -F'"' '/Apple Distribution/{print $2; exit}')"
[ -n "$SIGN_ID" ] || { echo "ERROR: no Apple Distribution identity in $KEYCHAIN — run apps/desktop/macos/setup-macos.sh first." >&2; exit 1; }
echo "=== app signing identity: $SIGN_ID (keychain: $KEYCHAIN) ==="

# Installer identities are not codesigning identities — list with the default
# policy (they never show up under -p codesigning).
INSTALLER_ID="$(security find-identity -v "$KEYCHAIN" \
    | awk -F'"' '/3rd Party Mac Developer Installer|Mac Installer Distribution/{print $2; exit}')"
[ -n "$INSTALLER_ID" ] || { echo "ERROR: no Mac Installer Distribution identity in $KEYCHAIN — run apps/desktop/macos/setup-macos.sh first." >&2; exit 1; }
echo "=== pkg signing identity: $INSTALLER_ID ==="

# ── Mac App Store provisioning profile (embedded in the .app below) ──────────
echo "=== Mac App Store provisioning profile ==="
PROFILE="$(ruby "$SCRIPT_DIR/mint-mas-profile.rb" - "$DX_BUNDLE_ID" "$PROFILE_NAME" \
    | awk -F= '/PROFILE_PATH=/{print $2}')"
[ -n "$PROFILE" ] && [ -f "$PROFILE" ] || { echo "ERROR: profile mint failed." >&2; exit 1; }
echo "profile: $PROFILE"

# ── Build ────────────────────────────────────────────────────────────────────
APP_GLOB="$ROOT/target/dx/$DX_PACKAGE/release/macos/"*.app
# shellcheck disable=SC2086
if [ "${SKIP_BUILD:-}" = "1" ] && [ -d $APP_GLOB ]; then
    echo "SKIP_BUILD=1 — reusing existing app"
else
    echo "=== building macOS app (dx, release) ==="
    # Remove any prior .app so a failed build can't be silently mistaken for a
    # fresh one (dx exits non-zero even on success, so we gate on the .app, not
    # the exit code — but only if it's THIS build's output).
    # shellcheck disable=SC2086
    rm -rf $APP_GLOB
    # No DEVELOPER_DIR override anywhere here: on the macOS-27 host, build
    # scripts must link the flake apple-sdk, not the system SDK (see the
    # airlock-ios skill; that trap bit the iOS flow's HOST build scripts).
    "$NIX" develop "$ROOT" --accept-flake-config -c bash -c "
        set -euo pipefail
        cd '$ROOT/$DX_APP_DIR'
        # DX_TAILWIND is relative to DX_APP_DIR. Build it from the input's
        # own directory: Tailwind v4's automatic content detection is rooted
        # at the working directory, so the wrong cwd silently drops rules.
        # Matches apps/tailwind_build.rs.
        ${DX_TAILWIND:+(cd \"\$(dirname '$DX_TAILWIND')\" && tailwindcss -i \"\$(basename '$DX_TAILWIND')\" -o '$ROOT/$DX_APP_DIR/assets/tailwind.css')}
        dx build --platform macos --release
    " > /tmp/task-macos-build.log 2>&1 || true
    tail -3 /tmp/task-macos-build.log
fi
# shellcheck disable=SC2086
APP="$(ls -d $APP_GLOB 2>/dev/null | head -1)"
[ -n "$APP" ] && [ -d "$APP" ] || { echo "ERROR: build produced no app"; tail -30 /tmp/task-macos-build.log; exit 1; }
PLIST="$APP/Contents/Info.plist"
echo "=== app bundle: $APP ==="

# ── Info.plist: identity, versions, category, SDK metadata ───────────────────
BUILD_NO="${BUILD_NO:-$(date +%s)}"
MARKETING_VER="${MARKETING_VER:-0.0.2}"
pb() { /usr/libexec/PlistBuddy -c "Set :$1 $2" "$PLIST" 2>/dev/null \
      || /usr/libexec/PlistBuddy -c "Add :$1 string $2" "$PLIST"; }
pb CFBundleIdentifier "$DX_BUNDLE_ID"
# dx names the bundle from the crate (TaskAppDesktop); the product is "Task".
pb CFBundleName "$PRODUCT_NAME"
pb CFBundleDisplayName "$PRODUCT_NAME"
pb CFBundleShortVersionString "$MARKETING_VER"
pb CFBundleVersion "$BUILD_NO"
pb CFBundlePackageType APPL
# dx stamps LSMinimumSystemVersion 10.15; require ≥12 for modern WKWebView.
pb LSMinimumSystemVersion 12.0
# App Store review requires a primary category on the macOS platform.
pb LSApplicationCategoryType public.app-category.productivity
# TestFlight requires the export-compliance declaration (standard TLS only).
/usr/libexec/PlistBuddy -c "Add :ITSAppUsesNonExemptEncryption bool false" "$PLIST" 2>/dev/null || true
pb NSHumanReadableCopyright "© FastTrackStudio"

# SDK build-metadata keys — Xcode's build injects these, dx does NOT, and
# App Store Connect rejects uploads it can't attribute to an SDK (the same
# 90534-class failure the iOS flow solved this way). Derived from the active
# Xcode's macOS platform; every read is ||-guarded so a missing key can't
# trip set -e.
DEV="${XCODE_DIR:-$(xcode-select -p)}"
SDK="$DEV/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk"
SDK_VER="$(/usr/libexec/PlistBuddy -c 'Print :Version' "$SDK/SDKSettings.plist" 2>/dev/null || true)"
SDK_BUILD="$(/usr/libexec/PlistBuddy -c 'Print :ProductBuildVersion' "$DEV/Platforms/MacOSX.platform/version.plist" 2>/dev/null || true)"
XCODE_ROOT="${DEV%/Contents/Developer}"
XCODE_BUILD="$(/usr/libexec/PlistBuddy -c 'Print :ProductBuildVersion' "$XCODE_ROOT/Contents/version.plist" 2>/dev/null || true)"
XCODE_VER="$(DEVELOPER_DIR="$DEV" xcodebuild -version 2>/dev/null | awk '/^Xcode/{print $2}' || true)"
# DTXcode = major*100 + minor*10 + patch, 4-digit (26.6 → 2660).
DTXCODE="$(echo "$XCODE_VER" | awk -F. '{printf "%02d%d%d", $1, ($2==""?0:$2), ($3==""?0:$3)}')"
MACOS_BUILD="$(sw_vers -buildVersion)"
pb DTPlatformName macosx
pb DTPlatformVersion "$SDK_VER"
pb DTPlatformBuild "$SDK_BUILD"
pb DTSDKName "macosx${SDK_VER}"
pb DTSDKBuild "$SDK_BUILD"
pb DTXcode "$DTXCODE"
pb DTXcodeBuild "$XCODE_BUILD"
pb DTCompiler "com.apple.compilers.llvm.clang.1_0"
pb BuildMachineOSBuild "$MACOS_BUILD"
echo "=== SDK metadata: macosx${SDK_VER} (${SDK_BUILD}), Xcode ${XCODE_VER} (${XCODE_BUILD}) ==="

# ── App icon (.icns generated from the shared 1024px master) ─────────────────
# dx references icon.icns in the plist but ships no icon; the App Store
# rejects icon-less apps. sips + iconutil are macOS built-ins, so unlike the
# iOS flow there is NO actool / Xcode-beta coupling here (that dance exists
# only because iOS icons must be an Assets.car).
# FAIL, don't warn. This used to `echo WARNING` and carry on, which meant a
# wrong ICON_1024 path produced a full signed build and a TestFlight upload
# that Apple rejected minutes later with "Missing required icon ... ICNS
# containing a 512pt x 512pt @2x image (90236)" — an error that says nothing
# about a path. The repo split moved this master and that is exactly what
# happened. An icon-less app can never ship, so stop here.
if [ ! -f "$ICON_1024" ]; then
    echo "ERROR: no icon master at $ICON_1024" >&2
    echo "       The App Store rejects icon-less apps (90236), so this build" >&2
    echo "       cannot ship. Set ICON_1024, or restore the 1024px master." >&2
    exit 1
fi
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
echo "=== app icon: AppIcon.icns (from $ICON_1024) ==="
echo "=== app: $PRODUCT_NAME ($DX_BUNDLE_ID) build $BUILD_NO ==="

# ── Entitlements: App Sandbox base + identity keys from the profile ──────────
# TestFlight / Mac App Store REQUIRES com.apple.security.app-sandbox (no
# hardened runtime here — that's the Developer-ID path). The committed base
# (Task.entitlements: sandbox, network-client, user-selected files — see its
# comments for the reasoning) is merged UNDER the profile's entitlements
# (com.apple.application-identifier, com.apple.developer.team-identifier):
# PlistBuddy Merge only adds keys absent from the target, so profile keys win.
security cms -D -i "$PROFILE" > /tmp/task-prof.plist
/usr/libexec/PlistBuddy -x -c "Print :Entitlements" /tmp/task-prof.plist > /tmp/task-ent.plist
/usr/libexec/PlistBuddy -c "Merge $SCRIPT_DIR/Task.entitlements" /tmp/task-ent.plist
echo "=== entitlements ==="
/usr/libexec/PlistBuddy -c Print /tmp/task-ent.plist

# ── Embed profile + sign (inside-out): nested code first, then the bundle ────
# macOS wants the profile at Contents/embedded.provisionprofile (iOS puts
# embedded.mobileprovision at the bundle root).
echo "=== signing (App Store distribution + App Sandbox) ==="
cp "$PROFILE" "$APP/Contents/embedded.provisionprofile"
find "$APP" \( -name "*.dylib" -o -name "*.so" -o -name "*.framework" \) -print0 \
    | while IFS= read -r -d '' f; do
        codesign --force --keychain "$KEYCHAIN" --timestamp --sign "$SIGN_ID" "$f"
      done
codesign --force --keychain "$KEYCHAIN" --timestamp \
    --entitlements /tmp/task-ent.plist --sign "$SIGN_ID" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# ── Installer .pkg (what actually gets uploaded for macOS) ───────────────────
echo "=== building signed installer pkg ==="
WORK="$(mktemp -d)"
PKG="$WORK/$PRODUCT_NAME-$MARKETING_VER-$BUILD_NO.pkg"
productbuild --component "$APP" /Applications \
    --sign "$INSTALLER_ID" --keychain "$KEYCHAIN" "$PKG"
pkgutil --check-signature "$PKG" | head -4
echo "pkg: $PKG"

if [ "${DRY_RUN:-}" = "1" ]; then
    echo "=== DRY_RUN=1 — stopping before upload. Signed pkg left at: $PKG ==="
    exit 0
fi

# ── Upload to TestFlight (same API-key mechanism as iOS; -t macos) ───────────
echo "=== uploading to TestFlight (macOS) ==="
xcrun altool --upload-app -t macos -f "$PKG" \
    --apiKey "$ASC_KEY_ID" --apiIssuer "$ASC_ISSUER_ID"
echo "=== DONE — build $BUILD_NO uploaded; it appears in TestFlight (macOS) after Apple processes it (~5-15 min) ==="
