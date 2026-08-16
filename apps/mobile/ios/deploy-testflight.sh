#!/usr/bin/env bash
# Build a release IPA and upload it to TestFlight via the App Store Connect
# API. The distribution cert is created via the API on first run, so no Xcode
# UI is needed. The App Store Connect key lives in ~/.appstoreconnect.
#
# Two ways to run:
#   Interactive Mac (login keychain, GUI session):
#     ./ios/deploy-testflight.sh
#   Headless build box (airlock) — run setup-keychain.sh once, then:
#     KEYCHAIN=fts-build.keychain KEYCHAIN_PW=fts-build ./ios/deploy-testflight.sh
#   (a dedicated keychain lets codesign work over SSH with no one logged in).
#
# Each upload needs a higher build number than the last; we stamp it from
# the current unix time (passed in, since dx/nix can't read the clock).
set -euo pipefail

# Derive paths from the script's own location, not git — a build box may hold
# an rsync'd source copy that isn't a git checkout.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
cd "$SCRIPT_DIR/.."

# Every scratch dir this script mktemp's, removed on exit (success OR failure).
# Each build stages a multi-hundred-MB watch DerivedData tree and an IPA
# Payload; leaking them accumulated ~13 GB of /var/folders junk on airlock and
# eventually failed builds outright with "No space left on device".
TMP_DIRS=()
cleanup_tmp() { [ ${#TMP_DIRS[@]} -eq 0 ] || rm -rf "${TMP_DIRS[@]}"; }
trap cleanup_tmp EXIT

# Which product to build+ship. Defaults to the FastTrackStudio app; override
# for Task (or any dx iOS app in the tree):
#   DX_PACKAGE=task-app-mobile DX_APP_DIR=apps/mobile DX_FEATURES="" \
#   ICONS_DIR=apps/mobile/ios/Assets.xcassets ./ios/deploy-testflight.sh
DX_PACKAGE="${DX_PACKAGE:-task-app-mobile}"
DX_APP_DIR="${DX_APP_DIR:-apps/mobile}"
# No colon: an explicitly-empty DX_FEATURES (Task, default features) is honored.
DX_FEATURES="${DX_FEATURES---no-default-features --features signal-guitar,signal-keys-rig}"
# Bundle id the App Store profile is minted for — must match the built .app's
# CFBundleIdentifier (from the package's Dioxus.toml).
DX_BUNDLE_ID="${DX_BUNDLE_ID:-app.fasttrackstudio.task}"
# Optional Tailwind input (relative to DX_APP_DIR) to compile → assets/tailwind.css
# before the build, so the embedded stylesheet isn't a stale stub (Task mobile).
DX_TAILWIND="${DX_TAILWIND:-}"

# Optional embedded watchOS companion. WATCH_APP = path (from repo root) to a
# watchos xcodegen project dir; when set, the watch app is built (xcodebuild,
# unsigned), embedded at <iOS.app>/Watch/<WATCH_PRODUCT>.app, and re-signed
# inside-out with the Apple Distribution cert + its own App Store profile, so
# it rides the iOS TestFlight build onto the paired watch and auto-updates.
#   WATCH_APP=apps/watchos ./ios/deploy-testflight.sh
WATCH_APP="${WATCH_APP:-}"
WATCH_SCHEME="${WATCH_SCHEME:-FTSWatch}"       # xcodebuild scheme
WATCH_PRODUCT="${WATCH_PRODUCT:-FastTrackStudio}"   # built .app product name
APP_DISPLAY_NAME="${APP_DISPLAY_NAME:-}"            # CFBundleDisplayName; unset = leave dx's
WATCH_BUNDLE_ID="${WATCH_BUNDLE_ID:-app.fasttrackstudio.task.watch}"

TEAM_ID="${TEAM_ID:-28C2G63DA7}"
# nix-darwin (airlock) and nixos put nix in different places; find it.
NIX="${NIX:-}"
if [ -z "$NIX" ]; then
    for c in /run/current-system/sw/bin/nix /nix/var/nix/profiles/default/bin/nix "$(command -v nix 2>/dev/null || true)"; do
        [ -n "$c" ] && [ -x "$c" ] && { NIX="$c"; break; }
    done
fi
[ -n "$NIX" ] || { echo "ERROR: nix not found (set NIX=...)." >&2; exit 1; }
BIN_IOS="$HOME/bin-ios"
mkdir -p "$BIN_IOS"
ln -sf /usr/bin/xcrun "$BIN_IOS/xcrun"

# shellcheck disable=SC1090
source "$HOME/.appstoreconnect/config.env"

# Signing keychain. On a headless box (airlock) set KEYCHAIN + KEYCHAIN_PW to
# the dedicated keychain that setup-keychain.sh provisioned — over SSH the
# login keychain can't be used for codesign (errSecInternalComponent). Default
# is the login keychain, which works when run from a GUI session (voyager).
KEYCHAIN="${KEYCHAIN:-login.keychain-db}"
KEYCHAIN_PW="${KEYCHAIN_PW:-}"
if [ -n "$KEYCHAIN_PW" ]; then
    security unlock-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN"
    security list-keychains -d user -s "$KEYCHAIN" login.keychain-db >/dev/null 2>&1 || true
fi

# Distribution signing identity — create it via the API + import if it isn't
# in the keychain yet (no Xcode UI needed).
if ! security find-identity -v -p codesigning "$KEYCHAIN" | grep -q "Apple Distribution"; then
    echo "=== creating Apple Distribution certificate ==="
    eval "$(ruby "$SCRIPT_DIR/mint-dist-cert.rb" | grep -E '^DIST_(KEY|CER)=')"
    # Bundle key + cert into a .p12 and import (-A: allow all apps, so
    # codesign uses it without a per-run keychain prompt). OpenSSL 3.x needs
    # -legacy for the encoding `security` reads; LibreSSL defaults to it.
    openssl x509 -inform DER -in "$DIST_CER" -out /tmp/fts-dist.pem
    if openssl pkcs12 -help 2>&1 | grep -q -- -legacy; then LEG="-legacy"; else LEG=""; fi
    # shellcheck disable=SC2086
    openssl pkcs12 -export $LEG -inkey "$DIST_KEY" -in /tmp/fts-dist.pem \
        -name "Apple Distribution" -out /tmp/fts-dist.p12 -passout pass:fts
    security import /tmp/fts-dist.p12 -k "$KEYCHAIN" -P fts -A -T /usr/bin/codesign
    rm -f /tmp/fts-dist.pem /tmp/fts-dist.p12
fi
SIGN_ID="$(security find-identity -v -p codesigning "$KEYCHAIN" \
    | awk -F'"' '/Apple Distribution/{print $2; exit}')"
[ -n "$SIGN_ID" ] || { echo "ERROR: distribution identity still missing after import." >&2; exit 1; }
echo "=== distribution identity: $SIGN_ID (keychain: $KEYCHAIN) ==="

echo "=== App Store provisioning profile ==="
PROFILE="$(PROFILE_TYPE=IOS_APP_STORE CERT_TYPE=DISTRIBUTION \
    ruby "$SCRIPT_DIR/mint-dev-profile.rb" - "$DX_BUNDLE_ID" | awk -F= '/PROFILE_PATH=/{print $2}')"
echo "profile: $PROFILE"

echo "=== building release ==="
# TestFlight needs a RELEASE Xcode/SDK. Point the build at a specific
# Xcode via XCODE_DIR (…/Xcode.app/Contents/Developer) — exporting
# DEVELOPER_DIR overrides both the nix apple-sdk env and xcode-select.
# Unset (default) uses whatever xcode-select points at.
if [ -n "${XCODE_DIR:-}" ]; then
    XCODE_ENV="export DEVELOPER_DIR='$XCODE_DIR'; unset SDKROOT"
    echo "using Xcode: $XCODE_DIR"
else
    XCODE_ENV="unset DEVELOPER_DIR SDKROOT"
fi
APP_GLOB="$ROOT/target/dx/$DX_PACKAGE/release/ios/"*.app
# dx can exit non-zero even on a successful build (and `| tail` + pipefail
# would then abort us), so capture to a log and gate on the .app instead.
# SKIP_BUILD=1 reuses an existing .app (fast iteration on the sign/upload path).
# shellcheck disable=SC2086
if [ "${SKIP_BUILD:-}" = "1" ] && [ -d $APP_GLOB ]; then
    echo "SKIP_BUILD=1 — reusing existing app"
else
    # DX_TAILWIND is relative to DX_APP_DIR. Tailwind v4's automatic content
    # detection is rooted at the WORKING DIRECTORY, so the sheet must be
    # built from the input's own directory or it silently loses rules —
    # hence the `cd $(dirname …)`. Keep in step with
    # apps/tailwind_build.rs, which pins the same cwd.
    "$NIX" develop "$ROOT" -c bash -c \
        "$XCODE_ENV; export PATH=$BIN_IOS:\$PATH; \
         cd '$ROOT/$DX_APP_DIR'; \
         ${DX_TAILWIND:+(cd \"\$(dirname '$DX_TAILWIND')\" \&\& tailwindcss -i \"\$(basename '$DX_TAILWIND')\" -o '$ROOT/$DX_APP_DIR/assets/tailwind.css');} \
         dx build --platform ios --device --release $DX_FEATURES" \
        > /tmp/fts-build.log 2>&1 || true
    tail -2 /tmp/fts-build.log
fi
# Resolve the actual .app (dx names it from the Dioxus.toml, per package).
# shellcheck disable=SC2086
APP="$(ls -d $APP_GLOB 2>/dev/null | head -1)"
[ -n "$APP" ] && [ -d "$APP" ] || { echo "ERROR: release build produced no app"; tail -25 /tmp/fts-build.log; exit 1; }
echo "=== app bundle: $APP ==="

BUNDLE="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Info.plist")"

# Info.plist: usage strings + file sharing + versions. The App Store
# requires CFBundleShortVersionString to be 1-3 period-separated integers
# (the crate's "0.0.1-alpha" is rejected); CFBundleVersion just has to climb.
BUILD_NO="${BUILD_NO:-$(date +%s)}"
MARKETING_VER="${MARKETING_VER:-0.0.1}"
# App bundle OS type — App Store requires CFBundlePackageType=APPL (dx omits it).
/usr/libexec/PlistBuddy -c "Set :CFBundlePackageType APPL" "$APP/Info.plist" 2>/dev/null \
    || /usr/libexec/PlistBuddy -c "Add :CFBundlePackageType string APPL" "$APP/Info.plist"
# Launch screen — required because iPad multitasking is implied by the
# orientation set. An empty UILaunchScreen dict = system default (fine).
/usr/libexec/PlistBuddy -c "Add :UILaunchScreen dict" "$APP/Info.plist" 2>/dev/null || true
# Single supported platform — dx leaves both iPhoneOS+iPadOS, which Apple
# rejects (91177). This is an iOS app; keep only iPhoneOS.
/usr/libexec/PlistBuddy -c "Delete :CFBundleSupportedPlatforms" "$APP/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :CFBundleSupportedPlatforms array" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Add :CFBundleSupportedPlatforms:0 string iPhoneOS" "$APP/Info.plist"
# Home-screen name. dx derives the bundle name from the Dioxus.toml
# `[application] name`, so `task-app-mobile` shipped as "TaskAppMobile" on
# the device. FTS only reads correctly because its package is already
# called `fasttrackstudio`. CFBundleDisplayName is what iOS actually shows,
# so set it explicitly rather than relying on the package name — the
# macOS script and both watch targets already do.
if [ -n "${APP_DISPLAY_NAME:-}" ]; then
    /usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName $APP_DISPLAY_NAME" "$APP/Info.plist" 2>/dev/null \
        || /usr/libexec/PlistBuddy -c "Add :CFBundleDisplayName string $APP_DISPLAY_NAME" "$APP/Info.plist"
fi
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $MARKETING_VER" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD_NO" "$APP/Info.plist"
# Minimum OS — App Store rejects a bundle without it (dx doesn't emit one).
/usr/libexec/PlistBuddy -c "Set :MinimumOSVersion 15.0" "$APP/Info.plist" 2>/dev/null \
    || /usr/libexec/PlistBuddy -c "Add :MinimumOSVersion string 15.0" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Add :NSMicrophoneUsageDescription string 'Processes your guitar signal from the connected audio interface or microphone.'" "$APP/Info.plist" 2>/dev/null || true
# Local network: pack downloads dial the studio engine peer-to-peer (iroh
# direct paths / LAN WebSocket); without this key iOS silently drops the
# traffic and the pack host is unreachable on the same Wi-Fi.
/usr/libexec/PlistBuddy -c "Add :NSLocalNetworkUsageDescription string 'Connects to your studio engine on the local network to stream and download sound packs.'" "$APP/Info.plist" 2>/dev/null || true
# The Bonjour type the app browses to RAISE the local-network prompt —
# iroh's raw UDP gets silently filtered instead of prompting without it.
/usr/libexec/PlistBuddy -c "Add :NSBonjourServices array" "$APP/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :NSBonjourServices:0 string _fts._tcp" "$APP/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :UIFileSharingEnabled bool true" "$APP/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :LSSupportsOpeningDocumentsInPlace bool true" "$APP/Info.plist" 2>/dev/null || true
# TestFlight requires ITSAppUsesNonExemptEncryption declared (false = no
# non-standard crypto → no export-compliance docs).
/usr/libexec/PlistBuddy -c "Add :ITSAppUsesNonExemptEncryption bool false" "$APP/Info.plist" 2>/dev/null || true

# SDK build-metadata keys. Xcode's build injects these; dx does NOT, and
# without them App Store Connect can't identify the SDK the binary was built
# against and rejects the upload as "unsupported SDK/Xcode" (error 90534).
# Derive them from the active Xcode/SDK so they stay correct across versions.
# All reads are `|| true`-guarded inside the command sub so a missing key
# can't trip set -e (a bare `var=$(failing)` assignment DOES exit under -e).
DEV="${XCODE_DIR:-$(xcode-select -p)}"
SDK="$DEV/Platforms/iPhoneOS.platform/Developer/SDKs/iPhoneOS.sdk"
SDK_VER="$(/usr/libexec/PlistBuddy -c 'Print :Version' "$SDK/SDKSettings.plist" 2>/dev/null || true)"
SDK_BUILD="$(/usr/libexec/PlistBuddy -c 'Print :ProductBuildVersion' "$DEV/Platforms/iPhoneOS.platform/version.plist" 2>/dev/null || true)"
XCODE_ROOT="${DEV%/Contents/Developer}"
XCODE_BUILD="$(/usr/libexec/PlistBuddy -c 'Print :ProductBuildVersion' "$XCODE_ROOT/Contents/version.plist" 2>/dev/null || true)"
XCODE_VER="$(DEVELOPER_DIR="$DEV" xcodebuild -version 2>/dev/null | awk '/^Xcode/{print $2}' || true)"
# DTXcode = major*100 + minor*10 + patch, 4-digit (26.6 → 2660).
DTXCODE="$(echo "$XCODE_VER" | awk -F. '{printf "%02d%d%d", $1, ($2==""?0:$2), ($3==""?0:$3)}')"
MACOS_BUILD="$(sw_vers -buildVersion)"
add_str() { /usr/libexec/PlistBuddy -c "Add :$1 string $2" "$APP/Info.plist" 2>/dev/null || \
            /usr/libexec/PlistBuddy -c "Set :$1 $2" "$APP/Info.plist" 2>/dev/null || true; }
add_str DTPlatformName iphoneos
add_str DTPlatformVersion "$SDK_VER"
add_str DTPlatformBuild "$SDK_BUILD"
add_str DTSDKName "iphoneos${SDK_VER}"
add_str DTSDKBuild "$SDK_BUILD"
add_str DTXcode "$DTXCODE"
add_str DTXcodeBuild "$XCODE_BUILD"
add_str DTCompiler "com.apple.compilers.llvm.clang.1_0"
add_str BuildMachineOSBuild "$MACOS_BUILD"
echo "=== SDK metadata: iphoneos${SDK_VER} (${SDK_BUILD}), Xcode ${XCODE_VER} (${XCODE_BUILD}) ==="

# Home-screen icon (dx emits none). actool compiles the asset catalog into
# Assets.car and writes CFBundleIcons/CFBundleIconName to a partial plist we
# merge. actool couples to the simulator SDK: on macOS 27 beta the system
# CoreSimulator wants an iOS 27 runtime that Xcode 26.6's simulator SDK can't
# supply, so 26.6's actool aborts before writing Assets.car (altool then
# rejects the upload: 90022/90023/91111). Point ACTOOL_DEVELOPER_DIR at an
# Xcode whose actool matches this OS (the 27 beta) for the icon compile ONLY —
# it produces a valid Assets.car and does not touch the build/upload SDK.
ICONS_DIR="${ICONS_DIR:-$ROOT/apps/mobile/ios/Assets.xcassets}"
# FAIL, don't skip. A missing ICONS_DIR used to fall straight through to the
# upload, which Apple then rejected (90022/90023/91111) — minutes later, with
# an error naming icon formats rather than the path that was wrong. The repo
# split moved this directory, so it is exactly the mistake that gets made.
if [ ! -d "$ICONS_DIR" ]; then
    echo "ERROR: no icon assets at $ICONS_DIR" >&2
    echo "       altool rejects icon-less apps, so this build cannot ship." >&2
    echo "       Set ICONS_DIR, or restore the Assets.xcassets." >&2
    exit 1
fi
    ICON_DEV="${ACTOOL_DEVELOPER_DIR:-$DEV}"
    ACTOOL="$ICON_DEV/usr/bin/actool"; [ -x "$ACTOOL" ] || ACTOOL="$(xcrun --find actool)"
    DEVELOPER_DIR="$ICON_DEV" "$ACTOOL" "$ICONS_DIR" --compile "$APP" --platform iphoneos \
        --minimum-deployment-target 15.0 --app-icon AppIcon \
        --output-partial-info-plist /tmp/fts-icon.plist >/tmp/fts-actool.log 2>&1 || true
    if [ -f "$APP/Assets.car" ] && /usr/libexec/PlistBuddy -c "Print :CFBundleIcons" /tmp/fts-icon.plist >/dev/null 2>&1; then
        /usr/libexec/PlistBuddy -c "Merge /tmp/fts-icon.plist" "$APP/Info.plist"
        # altool also wants a top-level CFBundleIconName (actool only nests it
        # under CFBundleIcons); add it defensively.
        /usr/libexec/PlistBuddy -c "Add :CFBundleIconName string AppIcon" "$APP/Info.plist" 2>/dev/null \
            || /usr/libexec/PlistBuddy -c "Set :CFBundleIconName AppIcon" "$APP/Info.plist" 2>/dev/null || true
        echo "app icon embedded (Assets.car + CFBundleIconName)"
    else
        # Also a hard failure. This printed ERROR and continued, so the upload
        # went ahead and Apple rejected it — the build was already doomed here,
        # several minutes earlier, with a far more legible message.
        echo "ERROR: app-icon compile produced no Assets.car — altool will reject." >&2
        echo "  set ACTOOL_DEVELOPER_DIR to an Xcode whose actool matches this macOS." >&2
        tail -6 /tmp/fts-actool.log >&2
        exit 1
    fi
echo "=== app: $APP ($BUNDLE) build $BUILD_NO ==="

# ── Embedded watchOS companion ───────────────────────────────────────────
# Build the watch app unsigned (xcodebuild), stamp its versions to match the
# host, embed at <iOS.app>/Watch/, and sign it inside-out with its OWN App
# Store profile. Must happen BEFORE the outer iOS app is signed (inside-out
# codesign order); the outer sign then seals the Watch/ subtree.
if [ -n "$WATCH_APP" ]; then
    echo "=== building watch companion ($WATCH_BUNDLE_ID) ==="
    WATCH_DIR="$ROOT/$WATCH_APP"
    # Regenerate the xcodeproj from project.yml (xcodegen from the flake registry).
    # Self-heal a stale nix eval cache: after a store GC the cached eval can hand
    # back a derivation whose `.drv` is gone ("don't know how to recreate store
    # derivation …xcodegen…"), which `nix run` alone won't recover from. On the
    # first failure, drop the eval cache and retry once so it re-evaluates fresh.
    run_xcodegen() { ( cd "$WATCH_DIR" && "$NIX" run nixpkgs#xcodegen -- generate ); }
    if ! run_xcodegen > /tmp/fts-watch-xcodegen.log 2>&1; then
        echo "xcodegen failed; clearing nix eval cache + retrying" | tee -a /tmp/fts-watch-xcodegen.log
        rm -rf "$HOME/.cache/nix"
        run_xcodegen >> /tmp/fts-watch-xcodegen.log 2>&1 \
            || { echo "ERROR: xcodegen failed"; tail -25 /tmp/fts-watch-xcodegen.log; exit 1; }
    fi
    WATCH_DD="$(mktemp -d)"
    TMP_DIRS+=("$WATCH_DD")
    # The watch build's Xcode is decoupled from the iOS build's: the dx iOS
    # Rust build's HOST build scripts fail to link under Xcode 27 beta
    # (ld: symbol(s) not found for arm64), so the main build must stay on the
    # GM, while the watch build can use whichever Xcode has the watchOS device
    # platform installed. Defaults to the main build's Xcode ($DEV).
    WATCH_DEV="${WATCH_XCODE_DIR:-$DEV}"
    # Unsigned generic-watchOS-device build; we re-sign manually below.
    DEVELOPER_DIR="$WATCH_DEV" xcodebuild \
        -project "$WATCH_DIR/$WATCH_SCHEME.xcodeproj" -scheme "$WATCH_SCHEME" \
        -configuration Release -destination 'generic/platform=watchOS' \
        -derivedDataPath "$WATCH_DD" \
        CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO \
        build > /tmp/fts-watch-build.log 2>&1 \
        || { echo "ERROR: watch build failed"; tail -30 /tmp/fts-watch-build.log; exit 1; }
    WATCH_BUILT="$WATCH_DD/Build/Products/Release-watchos/$WATCH_PRODUCT.app"
    [ -d "$WATCH_BUILT" ] || { echo "ERROR: no watch .app at $WATCH_BUILT"; tail -30 /tmp/fts-watch-build.log; exit 1; }

    echo "=== watch App Store provisioning profile ==="
    WATCH_PROFILE="$(PROFILE_TYPE=IOS_APP_STORE CERT_TYPE=DISTRIBUTION \
        ruby "$SCRIPT_DIR/mint-dev-profile.rb" - "$WATCH_BUNDLE_ID" "FTS Watch App Store" \
        | awk -F= '/PROFILE_PATH=/{print $2}')"
    echo "watch profile: $WATCH_PROFILE"

    # Embed at <iOS.app>/Watch/<product>.app (NOT PlugIns — modern companions).
    mkdir -p "$APP/Watch"
    rm -rf "$APP/Watch/$WATCH_PRODUCT.app"
    cp -R "$WATCH_BUILT" "$APP/Watch/"
    EMB="$APP/Watch/$WATCH_PRODUCT.app"
    # Version lockstep with the host app (companion + watch must agree, and
    # CFBundleVersion must climb per upload just like the host).
    /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $MARKETING_VER" "$EMB/Info.plist"
    /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD_NO" "$EMB/Info.plist"
    # actool compiles the AppIcon into Assets.car but doesn't always write the
    # icon key; altool wants CFBundleIconName. Add it (the catalog has AppIcon).
    /usr/libexec/PlistBuddy -c "Add :CFBundleIconName string AppIcon" "$EMB/Info.plist" 2>/dev/null \
        || /usr/libexec/PlistBuddy -c "Set :CFBundleIconName AppIcon" "$EMB/Info.plist"
    # Sign inside-out with the WATCH profile's entitlements.
    cp "$WATCH_PROFILE" "$EMB/embedded.mobileprovision"
    security cms -D -i "$WATCH_PROFILE" > /tmp/fts-watch-prof.plist
    /usr/libexec/PlistBuddy -x -c "Print :Entitlements" /tmp/fts-watch-prof.plist > /tmp/fts-watch-ent.plist
    find "$EMB" \( -name "*.dylib" -o -name "*.framework" \) -print0 \
        | while IFS= read -r -d '' f; do codesign --force --keychain "$KEYCHAIN" --sign "$SIGN_ID" --timestamp "$f"; done
    codesign --force --keychain "$KEYCHAIN" --sign "$SIGN_ID" \
        --entitlements /tmp/fts-watch-ent.plist --timestamp "$EMB"
    codesign --verify --strict "$EMB"
    echo "=== embedded + signed watch companion: $EMB ==="
fi

echo "=== signing (distribution) ==="
cp "$PROFILE" "$APP/embedded.mobileprovision"
security cms -D -i "$PROFILE" > /tmp/fts-prof.plist
/usr/libexec/PlistBuddy -x -c "Print :Entitlements" /tmp/fts-prof.plist > /tmp/fts-ent.plist
# Exclude the Watch/ subtree — it's already sealed; re-signing a nested
# framework would invalidate the watch app's signature.
find "$APP" \( -name "*.dylib" -o -name "*.framework" \) -not -path "$APP/Watch/*" -print0 \
    | while IFS= read -r -d '' f; do codesign --force --keychain "$KEYCHAIN" --sign "$SIGN_ID" --timestamp "$f"; done
codesign --force --keychain "$KEYCHAIN" --sign "$SIGN_ID" --entitlements /tmp/fts-ent.plist --timestamp "$APP"
codesign --verify --deep --strict "$APP"

echo "=== packaging IPA ==="
WORK="$(mktemp -d)"
TMP_DIRS+=("$WORK")
mkdir -p "$WORK/Payload"
cp -R "$APP" "$WORK/Payload/"
IPA="$WORK/FastTrackStudio.ipa"
# ditto (not zip) — preserves the _CodeSignature/CodeResources symlink that
# altool requires; a plain zip breaks it.
( cd "$WORK" && ditto -c -k --sequesterRsrc --keepParent Payload "$IPA" )

echo "=== uploading to TestFlight ==="
xcrun altool --upload-app -t ios -f "$IPA" \
    --apiKey "$ASC_KEY_ID" --apiIssuer "$ASC_ISSUER_ID"
echo "=== DONE — build $BUILD_NO uploaded; it appears in TestFlight after Apple processes it (~5-15 min) ==="
