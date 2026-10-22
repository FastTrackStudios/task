#!/usr/bin/env bash
# Attach the macOS app to the product release `v<version>` of
# FastTrackStudios/task: build the notarized Developer-ID app
# (build-dmg.sh), then upload, beside the node binaries, the web bundle and
# the Linux AppImage the same release carries,
#
#   Task-macos.dmg      — a first install: drag to Applications
#   Task-macos.zip      — what the in-app updater downloads
#   latest-macos.json   — the zip's version, URL and sha256 (the updater's
#                         manifest; apps/desktop/src/updater.rs)
#
# Installed copies find it on their own: the updater checks the releases
# API at launch and every few hours, verifies the download (sha256, our
# team's signature, Gatekeeper), and installs it on the next start or
# from the tray's "Check for updates".
#
#   just release-desktop 26.10.1
#
# The release itself is made by the deploy (release.yml, after the tag
# deploys and verifies live), so this waits for it — up to WAIT_MINUTES
# (default 180) — rather than creating one. Re-running replaces the three
# files (--clobber).
#
# Needs what build-dmg.sh needs (the Developer ID identity in the build
# keychain, ~/.appstoreconnect/config.env for notarization) and `gh`
# signed in with write access to the repo.
#
# PUBLISH=0 does everything up to the upload — build, sign, notarize,
# staple, the Gatekeeper check, the zip and manifest — and stops there.
# CI (task-macos.yml) builds with PUBLISH=0 and uploads itself, so it can
# also attach a build to an older tag whose copy of this script predates
# the single release.
set -euo pipefail

VERSION="${1:?usage: release.sh <version, e.g. 26.10.1>}"
# The release's CalVer version: three numbers, no zero padding.
[[ "$VERSION" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] \
    || { echo "version must be three numbers without leading zeros, e.g. 26.10.1" >&2; exit 2; }
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUT_DIR="${OUT_DIR:-$ROOT/target/dmg}"
TAG="v$VERSION"
PUBLISH="${PUBLISH:-1}"
WAIT_MINUTES="${WAIT_MINUTES:-180}"
REPO=FastTrackStudios/task

MARKETING_VER="$VERSION" OUT_DIR="$OUT_DIR" \
    KEYCHAIN="${KEYCHAIN:-fts-build.keychain}" KEYCHAIN_PW="${KEYCHAIN_PW:-fts-build}" \
    bash "$SCRIPT_DIR/build-dmg.sh"

for f in Task-macos.dmg Task-macos.zip latest-macos.json; do
    [ -f "$OUT_DIR/$f" ] || { echo "build-dmg.sh produced no $f" >&2; exit 1; }
done

if [ "$PUBLISH" != 1 ]; then
    echo "=== PUBLISH=$PUBLISH — built and notarized $VERSION, not uploading ==="
    cat "$OUT_DIR/latest-macos.json"
    exit 0
fi

echo "=== waiting for release $TAG (up to ${WAIT_MINUTES} min) ==="
waited=0
until gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; do
    [ "$waited" -lt "$WAIT_MINUTES" ] \
        || { echo "release $TAG did not appear in ${WAIT_MINUTES} min — has the deploy verified live?" >&2; exit 1; }
    sleep 60
    waited=$((waited + 1))
done

echo "=== attaching the macOS app to $TAG ==="
# The app first: the manifest is what makes installed apps move, so it
# must never name a zip that is not there yet.
gh release upload "$TAG" --repo "$REPO" --clobber "$OUT_DIR/Task-macos.dmg" "$OUT_DIR/Task-macos.zip"
gh release upload "$TAG" --repo "$REPO" --clobber "$OUT_DIR/latest-macos.json"
echo "=== attached: https://github.com/$REPO/releases/tag/$TAG ==="
