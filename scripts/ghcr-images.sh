#!/usr/bin/env bash
# Task's production images on GHCR, and the one rule that ties them to a
# release: the image tag is EXACTLY what the chart renders.
#
#   ghcr.io/fasttrackstudios/task-server:<appVersion>
#   ghcr.io/fasttrackstudios/task-web:<appVersion>
#
# starcommand's Argo CD `task` Application follows the latest `v*` tag of
# this repo and renders deploy/chart/task with `image.registry:
# ghcr.io/fasttrackstudios` and no tag override, so each image is
# `<registry>/<repo>:<Chart.yaml appVersion>` (templates/_helpers.tpl).
# The release commit sets appVersion to the release version (release.sh),
# so for vX.Y.Z the tag is `X.Y.Z`, no `v`.
#
#   scripts/ghcr-images.sh tag [REV]         the image tag at REV (default HEAD)
#   scripts/ghcr-images.sh check TAG [REV]   both images TAG exist and are
#                                            pullable WITHOUT credentials (as
#                                            the cluster pulls them); with REV,
#                                            both were built from commit REV
#                                            (label org.opencontainers.image.revision)
#
# `check` is anonymous on purpose: a private package fails it, and a
# private package is one the cluster cannot pull either.
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
REGISTRY="ghcr.io"
NAMESPACE="fasttrackstudios"
IMAGES=(task-server task-web)
CHART="deploy/chart/task/Chart.yaml"
MANIFEST_TYPES="application/vnd.docker.distribution.manifest.v2+json,application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.list.v2+json,application/vnd.oci.image.index.v1+json"

die() { echo "ghcr-images: $*" >&2; exit 1; }

cmd_tag() { # [rev]
    local c t
    c="$(git show "${1:-HEAD}:$CHART")" || die "no $CHART at ${1:-HEAD}"
    t="$(sed -n 's/^appVersion: *"\{0,1\}\([^"[:space:]]*\)"\{0,1\}.*/\1/p' <<<"$c" | head -1)"
    [ -n "$t" ] || die "$CHART at ${1:-HEAD} has no appVersion"
    echo "$t"
}

# GET a registry path with an anonymous pull token; prints the body.
registry_get() { # <image> <path> [accept]
    local img="$1" path="$2" accept="${3:-*/*}" tok
    tok="$(curl -fsS "https://$REGISTRY/token?scope=repository:$NAMESPACE/$img:pull" \
        | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')" || return 1
    [ -n "$tok" ] || return 1
    curl -fsSL -H "Authorization: Bearer $tok" -H "Accept: $accept" \
        "https://$REGISTRY/v2/$NAMESPACE/$img/$path"
}

cmd_check() { # <tag> [rev]
    local tag="${1:?usage: check TAG [REV]}" rev="${2:-}" img manifest digest cfg got ok=1
    [ -z "$rev" ] || rev="$(git rev-parse "$rev^{commit}")"
    for img in "${IMAGES[@]}"; do
        if ! manifest="$(registry_get "$img" "manifests/$tag" "$MANIFEST_TYPES" 2>/dev/null)"; then
            echo "ghcr-images: MISSING $REGISTRY/$NAMESPACE/$img:$tag (absent, or the package is not public)" >&2
            ok=0; continue
        fi
        if [ -n "$rev" ]; then
            # An index (multi-arch): its first image carries the labels.
            if grep -q '"manifests"' <<<"$manifest"; then
                digest="$(grep -o 'sha256:[0-9a-f]\{64\}' <<<"$manifest" | head -1 || true)"
                manifest="$(registry_get "$img" "manifests/$digest" "$MANIFEST_TYPES" 2>/dev/null || true)"
            fi
            # Registries may pretty-print; flatten before matching.
            manifest="$(tr -d ' \n\r\t' <<<"$manifest")"
            digest="$(grep -o '"config":{[^}]*}' <<<"$manifest" | grep -o 'sha256:[0-9a-f]*' | head -1 || true)"
            cfg="$(registry_get "$img" "blobs/$digest" 2>/dev/null | tr -d ' \n\r\t' || true)"
            got="$(grep -o '"org.opencontainers.image.revision":"[^"]*"' <<<"$cfg" | cut -d'"' -f4 || true)"
            if [ "$got" != "$rev" ]; then
                echo "ghcr-images: $img:$tag was built from ${got:-<no revision label>}, not $rev" >&2
                ok=0; continue
            fi
        fi
        echo "ghcr-images: ok $REGISTRY/$NAMESPACE/$img:$tag${rev:+ (revision $rev)}" >&2
    done
    [ "$ok" = 1 ]
}

cmd="${1:-}"; shift || true
case "$cmd" in
    tag) cmd_tag "$@" ;;
    check) cmd_check "$@" ;;
    *) sed -n '2,23p' "$0" >&2; exit 2 ;;
esac
