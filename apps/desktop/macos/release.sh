#!/usr/bin/env bash
# Publish the macOS app as a GitHub release of FastTrackStudio/task:
# build the notarized Developer-ID app (build-dmg.sh), then create the
# release `desktop-v<version>` carrying
#
#   Task-macos.dmg      — a first install: drag to Applications
#   Task-macos.zip      — what the in-app updater downloads
#   latest-macos.json   — the zip's version and sha256 (the updater's
#                         manifest; apps/desktop/src/updater.rs)
#
# Installed copies find it on their own: the updater checks the releases
# API at launch and every few hours, verifies the download (sha256, our
# team's signature, Gatekeeper), and installs it on the next start or
# from the tray's "Check for updates".
#
#   just release-desktop 0.0.3
#
# Needs what build-dmg.sh needs (the Developer ID identity in the build
# keychain, ~/.appstoreconnect/config.env for notarization) and `gh`
# signed in with write access to the repo.
set -euo pipefail

VERSION="${1:?usage: release.sh <version, e.g. 0.0.3>}"
case "$VERSION" in
    [0-9]*.[0-9]*.[0-9]*) ;;
    *) echo "version must be three numbers, e.g. 0.0.3" >&2; exit 2 ;;
esac
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUT_DIR="${OUT_DIR:-$ROOT/target/dmg}"
TAG="desktop-v$VERSION"

if gh release view "$TAG" --repo FastTrackStudios/task >/dev/null 2>&1; then
    echo "release $TAG already exists — pick the next version" >&2
    exit 1
fi

MARKETING_VER="$VERSION" OUT_DIR="$OUT_DIR" \
    KEYCHAIN="${KEYCHAIN:-fts-build.keychain}" KEYCHAIN_PW="${KEYCHAIN_PW:-fts-build}" \
    bash "$SCRIPT_DIR/build-dmg.sh"

for f in Task-macos.dmg Task-macos.zip latest-macos.json; do
    [ -f "$OUT_DIR/$f" ] || { echo "build-dmg.sh produced no $f" >&2; exit 1; }
done

echo "=== publishing $TAG ==="
gh release create "$TAG" \
    --repo FastTrackStudios/task \
    --target "$(git -C "$ROOT" rev-parse HEAD)" \
    --title "Task $VERSION for macOS" \
    --notes "Task $VERSION for macOS — signed, notarized.

First install: download **Task-macos.dmg**, open it, drag Task to Applications.
Already installed: Task updates itself (it checks in the background; the tray's *Check for updates* installs one now)." \
    "$OUT_DIR/Task-macos.dmg" "$OUT_DIR/Task-macos.zip" "$OUT_DIR/latest-macos.json"
echo "=== published: https://github.com/FastTrackStudios/task/releases/tag/$TAG ==="
