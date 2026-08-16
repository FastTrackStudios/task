#!/usr/bin/env bash
# One-time provisioning for Task macOS TestFlight builds on a headless box:
# make sure the dedicated build keychain holds BOTH identities the flow
# needs — "Apple Distribution" (signs the .app; shared with the iOS flow)
# and "Mac Installer Distribution" (signs the App Store .pkg wrapper).
#
# The keychain part mirrors the iOS setup-keychain.sh (see
# its comments for why a dedicated keychain is required over SSH); this
# script additionally mints whatever key/cert material is missing via the
# App Store Connect API helpers (mint-dist-cert.rb / mint-mac-installer.rb),
# so no Xcode UI or Developer-portal visit is ever needed.
#
# Idempotent: safe to re-run; existing identities are left untouched.
#
#   bash apps/desktop/macos/setup-macos.sh
#
# Env: KEYCHAIN (default fts-build.keychain), KEYCHAIN_PW (default fts-build).
# Needs ~/.appstoreconnect/config.env (ASC_KEY_ID/ASC_ISSUER_ID/ASC_KEY_PATH).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
IOS_DIR="$ROOT/apps/mobile/ios"

KEYCHAIN="${KEYCHAIN:-fts-build.keychain}"
KEYCHAIN_PW="${KEYCHAIN_PW:-fts-build}"
ASC="${ASC_DIR:-$HOME/.appstoreconnect}"
# shellcheck disable=SC1091
source "$ASC/config.env"

echo "=== keychain: $KEYCHAIN ==="
security create-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN" 2>/dev/null || echo "(already exists)"
# No auto-lock timeout. On a PRE-EXISTING keychain this can demand UI
# ("User interaction is not allowed" over SSH) — the original
# setup-keychain.sh already configured it, so tolerate the refusal.
security set-keychain-settings "$KEYCHAIN" 2>/dev/null \
    || echo "(set-keychain-settings needs UI — keeping existing settings)"
security unlock-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN"

# LibreSSL (macOS system openssl) has no -legacy flag and already defaults to
# the RC2/3DES PKCS#12 encoding that `security import` can read; OpenSSL 3.x
# needs -legacy for the same. Detect and branch. (A string, not an array —
# macOS bash 3.2 + `set -u` rejects expanding an empty array.)
if openssl pkcs12 -help 2>&1 | grep -q -- -legacy; then LEG="-legacy"; else LEG=""; fi

# import <key.pem> <cer.der> <name> [extra `security import` args...]
import_identity() {
    local key="$1" cer="$2" name="$3"; shift 3
    local P12 PEM
    P12="$(mktemp -t task-id).p12"
    PEM="$(mktemp -t task-id).pem"
    openssl x509 -inform DER -in "$cer" -out "$PEM"
    # shellcheck disable=SC2086
    openssl pkcs12 -export $LEG -inkey "$key" -in "$PEM" \
        -name "$name" -out "$P12" -passout pass:fts
    security import "$P12" -k "$KEYCHAIN" -P fts -A "$@"
    rm -f "$P12" "$PEM"
}

if security find-identity -v -p codesigning "$KEYCHAIN" | grep -q "Apple Distribution"; then
    echo "=== Apple Distribution identity already present ==="
else
    echo "=== obtaining Apple Distribution identity ==="
    ruby "$IOS_DIR/mint-dist-cert.rb" >/dev/null
    import_identity "$ASC/dist.key" "$ASC/dist.cer" "Apple Distribution" -T /usr/bin/codesign
fi

if security find-identity -v "$KEYCHAIN" | grep -Eq "3rd Party Mac Developer Installer|Mac Installer Distribution"; then
    echo "=== Mac Installer Distribution identity already present ==="
else
    echo "=== obtaining Mac Installer Distribution identity ==="
    ruby "$SCRIPT_DIR/mint-mac-installer.rb" >/dev/null
    # No -T: productbuild (not codesign) uses this one; -A covers it.
    import_identity "$ASC/mac-installer.key" "$ASC/mac-installer.cer" "Mac Installer Distribution"
fi

echo "=== Apple WWDR intermediates (needed to validate the chain) ==="
# A fresh keychain has no WWDR intermediate, so the leaf won't validate and
# `find-identity -v` shows nothing. Xcode normally installs these into login;
# fetch them straight from Apple so this box needs no Xcode UI ever.
for G in G3 G6; do
    CER="$(mktemp -t wwdr$G).cer"
    if curl -fsSL -o "$CER" "https://www.apple.com/certificateauthority/AppleWWDRCA$G.cer"; then
        security import "$CER" -k "$KEYCHAIN" 2>/dev/null || true
    fi
    rm -f "$CER"
done

echo "=== allow codesign to use the key without a UI prompt ==="
security set-key-partition-list -S apple-tool:,apple: -s -k "$KEYCHAIN_PW" "$KEYCHAIN" >/dev/null
# Put it on the search list (keep login too) so tools find the identity.
security list-keychains -d user -s "$KEYCHAIN" login.keychain-db >/dev/null 2>&1 || \
    security list-keychains -d user -s "$KEYCHAIN"

echo "=== result ==="
security find-identity -v -p codesigning "$KEYCHAIN" | grep -i "Apple Distribution" \
    || { echo "ERROR: no valid Apple Distribution identity after provisioning." >&2; exit 1; }
security find-identity -v "$KEYCHAIN" | grep -Ei "3rd Party Mac Developer Installer|Mac Installer Distribution" \
    || { echo "ERROR: no valid Mac Installer Distribution identity after provisioning." >&2; exit 1; }
echo "OK — both identities ready. Pass KEYCHAIN=$KEYCHAIN KEYCHAIN_PW=… to deploy-testflight-macos.sh"
