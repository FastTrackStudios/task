#!/usr/bin/env bash
# Build the macOS File Provider extension — the cloud folder on a Mac.
#
# Linux gets this behaviour from a FUSE mount the sync agent serves; on
# macOS the system loads an extension out of the app bundle and asks it
# for material instead. This builds that extension (an .appex) plus the
# small tool the app runs to register one File Provider domain per
# synced root, and drops both where `build-dmg.sh` expects to find them.
#
#   bash apps/desktop/macos/build-fileprovider.sh
#
# Env knobs (all optional):
#   OUT_DIR      where the .appex lands   (default: target/fileprovider)
#   TEAM_ID      signing team; unsigned build if absent
#   SIGN_ID      codesign identity        (default: "Apple Development")
#   TARGETS      rust targets to build    (default: both arches)
#   PROFILE      cargo profile            (default: release)
#   KEYCHAIN_PW  unlock the keychain first — needed when signing from an
#                SSH session, where codesign otherwise fails with the
#                unhelpful `errSecInternalComponent`
#   KEYCHAIN     which keychain to unlock (default: login.keychain-db)
#
# Deliberately not an Xcode project. The rest of this app is built by
# `dx` and shell, an .xcodeproj is a generated file nobody can review in
# a diff, and everything here is three swiftc invocations. When the
# extension grows a UI target that stops being true, and that is the
# point to reach for Xcode — not before.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"        # apps/desktop/macos
ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"         # repo root
SRC="$SCRIPT_DIR/FileProvider"

OUT_DIR="${OUT_DIR:-$ROOT/target/fileprovider}"
PROFILE="${PROFILE:-release}"
SIGN_ID="${SIGN_ID:-Apple Development}"
TARGETS="${TARGETS:-aarch64-apple-darwin x86_64-apple-darwin}"
BUNDLE_ID="app.fasttrackstudio.task.fileprovider"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "this builds a macOS extension; run it on a Mac" >&2
    exit 1
fi

echo "── the Rust half ───────────────────────────────────────────────"
# What Swift links: the stub format and the client that reaches the
# agent. Both architectures, then one universal static library, because
# an appex inside a universal app must be universal too.
#
# The deployment target is pinned to match the Swift side. Without it
# cargo builds for whatever macOS the build machine runs, and the link
# is a screenful of "built for newer macOS version than being linked" —
# noise that hides real warnings, from a mismatch that would eventually
# be a real one.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-12.0}"

LIBS=()
for target in $TARGETS; do
    rustup target add "$target" >/dev/null 2>&1 || true
    ( cd "$ROOT" && cargo build --"$PROFILE" -p files-fileprovider --target "$target" )
    LIBS+=("$ROOT/target/$target/$PROFILE/libfiles_fileprovider.a")
done

mkdir -p "$OUT_DIR"
LIB="$OUT_DIR/libfiles_fileprovider.a"
if [ "${#LIBS[@]}" -gt 1 ]; then
    lipo -create "${LIBS[@]}" -output "$LIB"
else
    cp "${LIBS[0]}" "$LIB"
fi
echo "   $(lipo -archs "$LIB")"

echo "── the extension ───────────────────────────────────────────────"
APPEX="$OUT_DIR/TaskFileProvider.appex"
rm -rf "$APPEX"
# Resources is empty and still required: codesign signs a bundle with a
# resource seal, and `codesign --verify --strict` on one with nowhere to
# put it fails with "code has no resources but signature indicates they
# must be present" — a sentence that names neither the bundle nor the
# missing directory.
mkdir -p "$APPEX/Contents/MacOS" "$APPEX/Contents/Resources"
cp "$SRC/Info.plist" "$APPEX/Contents/Info.plist"

HEADER="$ROOT/features/files/files-fileprovider/include/fts_fileprovider.h"

# `-import-objc-header` rather than a module map: this is the whole C
# surface, it is one header, and a bridging header is what the rest of
# the Apple toolchain expects to find in a project this size.
SWIFT_FLAGS=(
    -swift-version 5
    -target "$(uname -m)-apple-macos$MACOSX_DEPLOYMENT_TARGET"
    -import-objc-header "$HEADER"
    -framework FileProvider
    -framework Foundation
    # What the Rust half pulls in transitively: the TLS stack reads the
    # system trust store, and the QUIC/socket layer asks SystemConfiguration
    # about interfaces. Missing these is a wall of undefined `_kSC…`
    # symbols that says nothing about the cause.
    -framework SystemConfiguration
    -framework Security
    -framework CoreFoundation
    -Xlinker -force_load -Xlinker "$LIB"
)

# An appex has no `main`: the system calls `NSExtensionMain`, and
# without this the link fails on an undefined `_main` that looks like a
# missing source file rather than the wrong entry point.
swiftc "${SWIFT_FLAGS[@]}" \
    -module-name TaskFileProvider \
    -emit-executable \
    -Xlinker -e -Xlinker _NSExtensionMain \
    -o "$APPEX/Contents/MacOS/TaskFileProvider" \
    "$SRC/Bridge.swift" "$SRC/Item.swift" "$SRC/Tree.swift" \
    "$SRC/Enumerator.swift" "$SRC/FileProviderExtension.swift"

echo "── the domain registrar ────────────────────────────────────────"
# Only the containing app may register a domain, so this is a tool the
# app runs rather than anything the extension can do for itself.
swiftc "${SWIFT_FLAGS[@]}" \
    -module-name TaskFileProviderDomains \
    -emit-executable \
    -o "$OUT_DIR/TaskFileProviderDomains" \
    "$SRC/Bridge.swift" "$SRC/Tree.swift" "$SRC/Domains.swift"

echo "── signing ─────────────────────────────────────────────────────"
# An entitlements file is a plist, and codesign hands it to AMFI, whose
# parser answers a malformed one with "AMFIUnserializeXML: syntax error
# near line N" — no filename, and it does not stop the signature being
# written, so the first symptom is an extension the system silently
# refuses to load. XML forbids a double hyphen inside a comment, which
# is easy to write and impossible to see; `plutil -lint` says which file
# and which line, here, before any of that.
ENTITLEMENTS="$SRC/TaskFileProvider.entitlements"
plutil -lint "$ENTITLEMENTS" >/dev/null

# Signing needs the private key, and the login keychain is locked in a
# session with no window server — an SSH shell, a CI runner. codesign
# answers that with `errSecInternalComponent`, which says nothing about
# keychains at all. Same knob as build-dmg.sh, for the same reason.
if [ -n "${KEYCHAIN_PW:-}" ]; then
    security unlock-keychain -p "$KEYCHAIN_PW" "${KEYCHAIN:-login.keychain-db}"
fi

if [ -n "${TEAM_ID:-}" ]; then
    codesign --force --timestamp --options runtime \
        --entitlements "$ENTITLEMENTS" \
        --sign "$SIGN_ID" "$APPEX"
    codesign --force --timestamp --options runtime \
        --sign "$SIGN_ID" "$OUT_DIR/TaskFileProviderDomains"
    echo "   signed with $SIGN_ID"
else
    # Ad-hoc: enough to run the domain tool from a terminal and to
    # inspect the appex, not enough for the system to load it. The
    # system will not load an unsigned File Provider extension, and
    # saying so here is better than a silent no-op in Finder.
    codesign --force --sign - \
        --entitlements "$ENTITLEMENTS" "$APPEX" 2>/dev/null || true
    codesign --force --sign - "$OUT_DIR/TaskFileProviderDomains" 2>/dev/null || true
    echo "   ad-hoc only (no TEAM_ID) — macOS will not load this extension"
    echo "   set TEAM_ID and SIGN_ID to produce a loadable build"
fi

echo
echo "built:"
echo "   $APPEX"
echo "   $OUT_DIR/TaskFileProviderDomains"
echo
echo "the appex belongs at Task.app/Contents/PlugIns/TaskFileProvider.appex;"
echo "build-dmg.sh copies it there when it is present."
