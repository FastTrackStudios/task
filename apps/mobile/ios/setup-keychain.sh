#!/usr/bin/env bash
# Provision a DEDICATED codesigning keychain for headless TestFlight builds.
#
# A login keychain is bound to the GUI login session, so codesign over SSH
# fails with errSecInternalComponent until someone runs the app in a windowed
# session. A dedicated keychain we create, unlock, and set-partition-list on
# — all within the SSH session — sidesteps that entirely, which is what lets
# a headless build box (airlock) sign without anyone logged in at the screen.
#
# Idempotent: safe to re-run. Reads the distribution key/cert from
# ~/.appstoreconnect (dist.key + dist.cer, as written by mint-dist-cert.rb).
#
#   ./ios/setup-keychain.sh
#
# Env: KEYCHAIN (default fts-build.keychain), KEYCHAIN_PW (default fts-build).
set -euo pipefail

KEYCHAIN="${KEYCHAIN:-fts-build.keychain}"
KEYCHAIN_PW="${KEYCHAIN_PW:-fts-build}"
ASC="${ASC_DIR:-$HOME/.appstoreconnect}"
[ -f "$ASC/dist.key" ] && [ -f "$ASC/dist.cer" ] || {
    echo "ERROR: $ASC/dist.{key,cer} not found — run mint-dist-cert.rb first." >&2; exit 1; }

echo "=== keychain: $KEYCHAIN ==="
security create-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN" 2>/dev/null || echo "(already exists)"
security set-keychain-settings "$KEYCHAIN"           # no auto-lock timeout
security unlock-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN"

echo "=== importing distribution identity ==="
# LibreSSL (macOS system openssl) has no -legacy flag and already defaults to
# the RC2/3DES PKCS#12 encoding that `security import` can read; OpenSSL 3.x
# needs -legacy for the same. Detect and branch.
P12="$(mktemp -t fts-dist).p12"
PEM="$(mktemp -t fts-dist).pem"
openssl x509 -inform DER -in "$ASC/dist.cer" -out "$PEM"
if openssl pkcs12 -help 2>&1 | grep -q -- -legacy; then LEG=(-legacy); else LEG=(); fi
openssl pkcs12 -export "${LEG[@]}" -inkey "$ASC/dist.key" -in "$PEM" \
    -name "Apple Distribution" -out "$P12" -passout pass:fts
security import "$P12" -k "$KEYCHAIN" -P fts -A -T /usr/bin/codesign
rm -f "$P12" "$PEM"

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
    || { echo "ERROR: no valid distribution identity after provisioning." >&2; exit 1; }
echo "OK — headless codesigning ready. Pass KEYCHAIN=$KEYCHAIN KEYCHAIN_PW=… to deploy-testflight.sh"
