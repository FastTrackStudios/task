#!/usr/bin/env bash
# Exercise the File Provider extension without building the whole app.
#
# A File Provider domain may only be registered by a signed app that
# contains the extension — which normally means building Task.app, which
# means dx, a web build and a nix shell. That is a long way to go to
# answer one question, and the question is the risky part of the whole
# design: **will the system load this extension, and can it read the
# tree.**
#
# So this builds the smallest thing that can answer it — an app bundle
# whose only executable is the domain registrar, with the extension in
# PlugIns — signs both, and registers a domain. What Finder then shows
# (or refuses to) is the real answer.
#
#   TEAM_ID=… SIGN_ID="Developer ID Application: …" \
#     KEYCHAIN=fts-build.keychain KEYCHAIN_PW=… \
#     bash apps/desktop/macos/try-fileprovider.sh
#
# It is a harness, not a product: the bundle it makes is not the app,
# has no UI, and is meant to be thrown away (`clear` unregisters).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
SRC="$SCRIPT_DIR/FileProvider"
OUT_DIR="${OUT_DIR:-$ROOT/target/fileprovider}"
APP="$OUT_DIR/TaskFileProviderHarness.app"
# The same bundle id the real app uses: a File Provider extension's id
# must be a prefix-child of its container's, and the entitlements are
# written against this one.
BUNDLE_ID="${DX_BUNDLE_ID:-app.fasttrackstudio.task}"

[ "$(uname -s)" = "Darwin" ] || { echo "macOS only" >&2; exit 1; }

bash "$SCRIPT_DIR/build-fileprovider.sh"

echo "── the harness bundle ──────────────────────────────────────────"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/PlugIns"
cp "$OUT_DIR/TaskFileProviderDomains" "$APP/Contents/MacOS/TaskFileProviderHarness"
cp -R "$OUT_DIR/TaskFileProvider.appex" "$APP/Contents/PlugIns/"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>TaskFileProviderHarness</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleName</key><string>Task</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.0.2</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>12.0</string>
</dict>
</plist>
PLIST

echo "── signing ─────────────────────────────────────────────────────"
[ -n "${KEYCHAIN_PW:-}" ] && security unlock-keychain -p "$KEYCHAIN_PW" \
    "${KEYCHAIN:-login.keychain-db}"
SIGN_ID="${SIGN_ID:-Apple Development}"

# Inside out: signing anything nested after the bundle invalidates the
# outer signature.
codesign --force --timestamp --options runtime \
    ${KEYCHAIN:+--keychain "$KEYCHAIN"} \
    --entitlements "$SRC/TaskFileProvider.entitlements" \
    --sign "$SIGN_ID" "$APP/Contents/PlugIns/TaskFileProvider.appex"
codesign --force --timestamp --options runtime \
    ${KEYCHAIN:+--keychain "$KEYCHAIN"} \
    --entitlements "$SCRIPT_DIR/Task-devid.entitlements" \
    --sign "$SIGN_ID" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

echo
echo "── telling the system the extension exists ─────────────────────"
# An installed app is discovered by LaunchServices; a bundle sitting in
# a build directory is not, and `NSFileProviderManager.add` then refuses
# with "The application cannot be used right now" — which names neither
# the extension nor the reason. Registering it by hand is the harness's
# job, and re-registering after every rebuild is required: the record
# points at a signature that no longer exists.
pluginkit -a "$APP/Contents/PlugIns/TaskFileProvider.appex"
sleep 2

echo "── registering a domain ────────────────────────────────────────"
# The moment of truth. An extension the system declines to load, or a
# container it will not trust, shows up here rather than in Finder.
"$APP/Contents/MacOS/TaskFileProviderHarness" "${1:-sync}"

echo
echo "harness: $APP"
echo "  what it registered:  $APP/Contents/MacOS/TaskFileProviderHarness list"
echo "  take it back down:   $APP/Contents/MacOS/TaskFileProviderHarness clear"
