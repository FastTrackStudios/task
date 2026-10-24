#!/usr/bin/env bash
# Task releases: one version, one changelog, one tag (fleet flow v4).
#
# The fleet standard is the `release-workflow` skill (fleet-skills,
# skills/release-workflow/: SKILL.md + references/). This script is Task's
# implementation of it; where the two disagree, the skill wins and this
# file is stale.
#
#   short-lived branch --PR, squash--> main --release PR (one commit)--> tag vYY.M.MICRO
#                                       │                                    │
#                  full suite after every merge                 THE TAG DEPLOYS: server +
#                                                               web, TestFlight (iOS, macOS),
#                                                               node binaries, GitHub Release
#
#   hotfix while main can't ship: release/v<latest tag> (from the tag)
#       <-- cherry-pick -x from main, PR --> release commit --> tag (next CalVer)
#       then a forward-port PR copies the changelog section into main.
#
# `main` is the only long-lived branch. Production is the latest `v*` tag.
#
# VERSIONS are CalVer YY.M.MICRO (no zero padding: Cargo rejects `26.01.0`).
# The next version is computed, never chosen: take the highest release tag
# (any branch, `-` suffixes excluded); same YY.M as today -> MICRO+1, else
# YY.M.0 of the current month. Hotfixes use the same rule, so every version
# is unique and ascending (the App Store requires that).
#
# The version lives in ONE place, `[workspace.package] version` in the
# root Cargo.toml. Every shipped thing reads it from there: task-server
# and the web bundle through CARGO_PKG_VERSION, the iPhone and macOS
# TestFlight builds through their workflows (MARKETING_VER), and the
# `v<version>` git tag + GitHub Release cut by the tag's deploy, which
# then carries everything the version ships: the standalone headless node
# binaries (node-binaries.yml), the web bundle and the Linux AppImage
# (release-assets.yml), and the Developer-ID macOS app (task-macos.yml) —
# the desktop downloads with the manifests the in-app updater follows.
# (Until 26.10 the macOS app had its own `desktop-v<version>` release.)
# scripts/check-versions.sh (`just ci-versions`) holds every one of them
# to it.
#
# The CHANGELOG section is built from the pending cargo-rail change files
# (`.changes/*.md`, one per PR, written by `cargo rail change add`), which
# the release commit deletes. Only when there are none at all does it fall
# back to git-cliff over the commit range, so a release is never empty.
#
#   scripts/release.sh version [REV]       the current version (or at REV)
#   scripts/release.sh next                the version the next release gets
#   scripts/release.sh plan                latest tag, what main holds since, pending
#                                           change files, migrations, moved git deps
#   scripts/release.sh prepare [X.Y.Z] [--on release/vA.B.C]
#                                          cut release/pending from origin/main (or the
#                                           hotfix branch): bump + changelog, ONE commit
#   scripts/release.sh pr                  push release/pending, open its PR (squash)
#   scripts/release.sh tag [VERSION]       after the release PR merged and its full
#                                           suite is green: tag v<VERSION>
#   scripts/release.sh hotfix NAME SHA...  release/v<latest tag> + cherry-pick -x of
#                                           SHA... from main into fix/NAME, PR into it
#   scripts/release.sh forward-port VER    PR into main: VER's changelog section, and
#                                           delete the change files it consumed
#   scripts/release.sh notes [VERSION]     one version's changelog section
#   scripts/release.sh guard               CI: is this release PR sound? (others: no-op)
#   scripts/release.sh release-only        CI (checks): is this PR only a release
#                                           commit? (exit 0 = yes)
#   scripts/release.sh tag-guard           CI (deploy): is this tag a release?
#   scripts/release.sh publish             CI: GitHub Release for the tag
#   scripts/release.sh migrations [A] [B]  migrations added in A..B
#
# See skills/release-workflow.md for Task's specifics.
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
REPO="${GITHUB_REPOSITORY:-FastTrackStudios/task}"
MAIN="${MAIN_BRANCH:-main}"
# The head of every release PR (into main, or into release/v* for a
# hotfix). One fixed name: the guard and release-only key on it, and two
# releases can never be in flight at once.
RELEASE_BRANCH="release/pending"
# The full suite's verdict on a main commit (checks.yml's aggregator job,
# `CI passed`). `tag` refuses a commit on which it is not green.
CHECK_NAME="CI passed"
# The Helm chart: the release commit sets its version and appVersion, and
# appVersion is the image tag the chart at the tag renders
# (scripts/ghcr-images.sh).
CHART="deploy/chart/task/Chart.yaml"

die() { echo "release: $*" >&2; exit 1; }
note() { echo "release: $*" >&2; }

# git-cliff from PATH, else from nixpkgs (it is not in the dev shell, and
# adding it there would make every flake.lock bump an infra change). Only
# the no-change-files fallback of the changelog uses it.
cliff() {
    if command -v git-cliff >/dev/null 2>&1; then
        git-cliff "$@"
    elif command -v nix >/dev/null 2>&1; then
        nix run --accept-flake-config nixpkgs#git-cliff -- "$@"
    else
        die "git-cliff not found (and no nix to fetch it)"
    fi
}

# `[workspace.package] version` of Cargo.toml at a revision (default: the
# working tree).
version_at() {
    local src
    if [ -n "${1:-}" ]; then src="$(git show "$1:Cargo.toml")"; else src="$(cat Cargo.toml)"; fi
    printf '%s\n' "$src" \
        | sed -n '/^\[workspace\.package\]/,/^\[/{s/^version *= *"\([^"]*\)".*/\1/p;}' \
        | head -1
}

is_semver() { [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; }

# The release commit's subject for a version, as an ERE (a squash merge
# appends " (#N)").
release_subject_re() { printf '^chore\\(release\\): v%s( \\(#[0-9]+\\))?$' "${1//./\\.}"; }

# true when $1 > $2 (both X.Y.Z). sort -V, never a string compare:
# 26.10.10 > 26.10.9.
gt() { [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -1)" = "$1" ]; }

# Product tags only: exactly vX.Y.Z. `desktop-v*` (the macOS app's own
# releases before 26.10) and any `-rc.N` style suffix never match. versionsort.suffix=- sorts a
# suffixed tag before its release, should one ever exist.
tags_sorted() { # [--merged REF]
    git -c versionsort.suffix=- tag --list 'v[0-9]*' "$@" --sort=-v:refname \
        | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' || true
}

# The newest release tag reachable from REF (default HEAD): the range start
# for migrations and moved git deps.
last_tag() { tags_sorted --merged "${1:-HEAD}" | head -1; }

# The highest release tag anywhere (main or a release/v* hotfix branch):
# what the next version counts from.
latest_tag() { tags_sorted | head -1; }

# Today as YYYY-MM-DD (UTC). RELEASE_DATE overrides it, for tests.
today() { echo "${RELEASE_DATE:-$(date -u +%F)}"; }

# The CalVer version after <latest> on <date>: same YY.M -> MICRO+1, else
# YY.M.0. A pre-CalVer latest (0.2.4) is simply "another month".
calver_next() { # <latest X.Y.Z or empty> [YYYY-MM-DD]
    local latest="${1:-}" d="${2:-$(today)}" yy mm ma mi pa next
    [[ "$d" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || die "bad date '$d' (want YYYY-MM-DD)"
    yy=$((10#${d:2:2})); mm=$((10#${d:5:2}))
    if [ -n "$latest" ]; then
        IFS=. read -r ma mi pa <<<"$latest"
        if [ "$ma" = "$yy" ] && [ "$mi" = "$mm" ]; then next="$yy.$mm.$((pa + 1))"; else next="$yy.$mm.0"; fi
        gt "$next" "$latest" || die "the next version $next would not sort above the latest tag v$latest (a tag from the future?)"
    else
        next="$yy.$mm.0"
    fi
    echo "$next"
}

# Fetch what every release decision reads. A failure is fatal: a guard or
# a tag that can't see origin must not act on stale refs.
fetch_release_refs() {
    git fetch --quiet --tags --force origin \
        "+refs/heads/$MAIN:refs/remotes/origin/$MAIN" \
        "+refs/heads/release/*:refs/remotes/origin/release/*" \
        || die "git fetch from origin failed: refusing to decide on stale refs"
}

# Sea-orm migrations added in a range — the thing that makes a rollback a
# data restore instead of an image swap. Release notes carry it.
migrations_between() { # <from> <to>
    # Migrations here are modules inside each service's migrations.rs
    # (`mod m20261012_000001_create_activity`, registered with
    # `Box::new(m…::Migration)`, sometimes named by a string literal), so
    # look for ADDED lines carrying such a name rather than for added files.
    #
    # BLIND SPOT: migrations that arrive inside a git dependency (a
    # Cargo.lock bump of architect & co. — m20261019_000001_create_devices
    # came that way) are not in this repo's diff. The authoritative list is
    # the server's own `seaql_migrations` tables after deploy; the release
    # notes flag lockfile bumps of git deps so a human looks.
    [ -n "$1" ] || return 0
    git diff -U0 "$1" "$2" -- '*.rs' \
        | grep -E '^\+' \
        | grep -oE '\bm[0-9]{8}_[0-9]{6}_[A-Za-z0-9_]+' | sort -u || true
}

# Git dependencies whose locked commit moved in a range — where migrations
# (and behaviour) can arrive without a line of this repo's source changing.
git_deps_moved() { # <from> <to>
    [ -n "$1" ] || return 0
    git diff -U0 "$1" "$2" -- Cargo.lock \
        | grep -E '^\+source = "git\+' \
        | sed -E 's/^\+source = "git\+([^?#"]+).*/\1/' | sort -u || true
}

# ── Change files (.changes/*.md, written by `cargo rail change add`) ───────
#
#   ---
#   "task-server" = "patch"
#   "files-store" = "minor"
#   ---
#
#   Fixed sync stalling after offline edits
#
# The file's highest bump decides its changelog group. `.changes/README`
# has no .md extension on purpose: rail parses every .md there.

# Pending change files at a revision (default: the working tree), sorted.
change_files() { # [rev]
    if [ -n "${1:-}" ]; then
        git ls-tree --name-only "$1" .changes/ 2>/dev/null | grep -E '^\.changes/[^/]+\.md$' | sort || true
    else
        find .changes -maxdepth 1 -type f -name '*.md' 2>/dev/null | sort || true
    fi
}

# The highest bump named in a change file's front matter: major, minor,
# patch or none. Dies on a file rail would not accept.
change_bump() { # <file>
    awk -v f="$1" '
        NR == 1 { if ($0 != "---") { bad = "no front matter"; exit } fm = 1; next }
        fm && $0 == "---" { closed = 1; exit }
        fm && /^[[:space:]]*(#|$)/ { next }
        fm {
            if (!match($0, /=[[:space:]]*"(none|patch|minor|major)"[[:space:]]*$/)) { bad = "bad line: " $0; exit }
            b = substr($0, RSTART, RLENGTH); gsub(/[^a-z]/, "", b)
            r = (b == "major") ? 3 : (b == "minor") ? 2 : (b == "patch") ? 1 : 0
            if (r > best) best = r; n++
        }
        END {
            if (bad == "" && !closed) bad = "unterminated front matter"
            if (bad == "" && n == 0) bad = "names no crate"
            if (bad != "") { print "release: invalid change file " f ": " bad > "/dev/stderr"; exit 1 }
            split("none patch minor major", name, " "); print name[best + 1]
        }' "$1"
}

# A change file's body as one markdown list item: "- first line", the rest
# indented two spaces, leading/trailing blank lines dropped.
change_item() { # <file>
    awk '
        NR == 1 { next }                           # opening ---
        !body && $0 == "---" { body = 1; next }    # closing ---
        !body { next }
        { line[++n] = $0 }
        END {
            s = 1; while (s <= n && line[s] ~ /^[[:space:]]*$/) s++
            e = n; while (e >= s && line[e] ~ /^[[:space:]]*$/) e--
            for (i = s; i <= e; i++) {
                if (i == s) print "- " line[i]
                else if (line[i] ~ /^[[:space:]]*$/) print ""
                else print "  " line[i]
            }
        }' "$1"
}

# The CHANGELOG section for <version> from the pending change files:
# major -> Breaking / Migration, minor -> Added, patch -> Fixed and
# improved, none -> skipped. Prints nothing when there are no change files
# at all (the caller falls back to git-cliff).
changes_section() { # <version> <date> [preamble line]
    local v="$1" d="$2" pre="${3:-}" f b any=0 major="" minor="" patch=""
    local files; files="$(change_files)"
    [ -n "$files" ] || return 0
    while read -r f; do
        b="$(change_bump "$f")" || die "fix $f (cargo rail change status shows the same)"
        case "$b" in
            major) major+="$(change_item "$f")"$'\n'; any=1 ;;
            minor) minor+="$(change_item "$f")"$'\n'; any=1 ;;
            patch) patch+="$(change_item "$f")"$'\n'; any=1 ;;
            none) ;;
        esac
    done <<<"$files"
    echo "## [$v] - $d"
    echo
    [ -z "$pre" ] || { echo "$pre"; echo; }
    [ -z "$major" ] || printf '### Breaking / Migration\n\n%s\n' "$major"
    [ -z "$minor" ] || printf '### Added\n\n%s\n' "$minor"
    [ -z "$patch" ] || printf '### Fixed and improved\n\n%s\n' "$patch"
    [ "$any" = 1 ] || printf -- '- No user-visible changes.\n\n'
}

cmd_version() { version_at "${1:-}"; }

cmd_next() {
    git fetch --quiet --tags origin 2>/dev/null || note "fetch failed; computing from local tags"
    local t; t="$(latest_tag)"
    calver_next "${t#v}"
}

cmd_notes() {
    local v="${1:-$(version_at)}"
    awk -v want="## [$v]" '
        index($0, want) == 1 { on = 1; next }
        on && /^## \[/ { exit }
        on { print }
    ' CHANGELOG.md | sed -e '/./,$!d' # drop leading blank lines
}

# `git rev-list --count --no-merges A..B`, tolerant of a missing A.
count_between() {
    if [ -n "${1:-}" ]; then git rev-list --count --no-merges "$1..$2"; else git rev-list --count --no-merges "$2"; fi 2>/dev/null || echo 0
}

cmd_plan() {
    fetch_release_refs
    local ref="origin/$MAIN" cur tag latest next files f
    cur="$(version_at "$ref")"; tag="$(last_tag "$ref")"; latest="$(latest_tag)"
    next="$(calver_next "${latest#v}")"
    echo "production (latest v* tag):  ${latest:-none}"
    [ "$latest" = "$tag" ] || echo "latest tag on $MAIN:          ${tag:-none} (${latest} is a hotfix on release/${tag})"
    echo "version on $MAIN:             $cur"
    echo "on $MAIN since ${tag:-the start}:     $(count_between "$tag" "$ref") commits"
    echo "next release:                $next (CalVer: computed, not chosen)"
    echo
    files="$(git ls-tree --name-only "$ref" .changes/ 2>/dev/null | grep -E '\.md$' || true)"
    echo "── pending change files on $MAIN ──"
    if [ -n "$files" ]; then
        while read -r f; do
            echo "$f ($(git show "$ref:$f" | change_bump /dev/stdin 2>/dev/null || echo invalid))"
        done <<<"$files"
    else
        echo "none: the release falls back to git-cliff over ${tag:+$tag..}$MAIN"
    fi
    echo
    echo "── commits on $MAIN since ${tag:-the start} ──"
    git log --no-merges --format='- %s' ${tag:+"$tag..$ref"} | head -100
    if [ -n "$tag" ]; then
        local m; m="$(migrations_between "$tag" "$ref")"
        echo "── migrations since $tag ──"
        echo "${m:-none}"
        local d; d="$(git_deps_moved "$tag" "$ref")"
        echo "── git dependencies moved since $tag (their migrations are not listed above) ──"
        echo "${d:-none}"
    fi
}

# Cut release/pending from origin/main (or, with --on, from a hotfix
# branch release/vX.Y.Z): ONE commit `chore(release): v<next>` holding the
# version bump, the Cargo.lock version lines, the new CHANGELOG section and
# the deletion of the change files it consumed.
cmd_prepare() {
    local want="" on="$MAIN" cur latest next computed tag base pre=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --on) on="${2:?--on needs a branch}"; shift 2 ;;
            --on=*) on="${1#--on=}"; shift ;;
            *) [ -z "$want" ] || die "usage: release.sh prepare [X.Y.Z] [--on release/vA.B.C]"; want="${1#v}"; shift ;;
        esac
    done
    [ "$on" = "$MAIN" ] || [[ "$on" =~ ^release/v[0-9]+\.[0-9]+\.[0-9]+$ ]] \
        || die "--on takes $MAIN or a hotfix branch release/vX.Y.Z, not '$on'"
    [ -z "$(git status --porcelain --untracked-files=no)" ] || die "working tree is dirty"
    fetch_release_refs
    base="origin/$on"
    git rev-parse -q --verify "$base" >/dev/null || die "no $base (just release-hotfix creates a hotfix branch)"
    if git ls-remote --exit-code --heads origin "$RELEASE_BRANCH" >/dev/null 2>&1; then
        die "$RELEASE_BRANCH already exists on origin: a release is in flight. Finish it (merge, just release-tag), or close its PR and delete the branch (git push origin --delete $RELEASE_BRANCH) to re-cut."
    fi

    cur="$(version_at "$base")"
    is_semver "$cur" || die "Cargo.toml version '$cur' is not X.Y.Z"
    # A release commit already merged but never tagged: finish that one.
    git rev-parse -q --verify "refs/tags/v$cur" >/dev/null \
        || die "$base is at $cur but there is no tag v$cur: an earlier release was merged and not tagged. Finish it: just release-tag $cur"
    tag="$(last_tag "$base")"
    [ "$(count_between "$tag" "$base")" -gt 0 ] || die "nothing to release: $on has no commits since $tag"

    latest="$(latest_tag)"
    next="$(calver_next "${latest#v}")"; computed="$next"
    if [ -n "$want" ] && [ "$want" != "$next" ]; then
        is_semver "$want" || die "VERSION must be X.Y.Z"
        gt "$want" "${latest#v}" || die "$want does not sort above the latest tag $latest"
        note "explicit version $want instead of the computed $next: the commit carries a Release-Version-Override trailer for the guard"
        next="$want"
    fi
    git rev-parse -q --verify "refs/tags/v$next" >/dev/null && die "tag v$next already exists"

    # main after a hotfix: say what the previous hotfix already shipped.
    [ "$on" != "$MAIN" ] || [ "$latest" = "$tag" ] || pre="Includes all changes up to $latest."

    git switch --quiet -C "$RELEASE_BRANCH" "$base"
    note "cutting $RELEASE_BRANCH: v$cur -> v$next on $on $(git rev-parse --short "$base")"
    release_commit "$cur" "$next" "$tag" "$pre" "$([ "$next" = "$computed" ] || echo "$computed")"
    cat >&2 <<EOT

Next:
  1. Read CHANGELOG.md's [$next] section; tidy the wording in this commit
     if needed (git commit --amend; it must stay ONE commit).
  2. just release-pr      pushes $RELEASE_BRANCH, opens "chore(release): v$next" -> $on.
     The release guard runs; the heavy checks are skipped (version + changelog only).
  3. Squash-merge it while $on has not moved (if it moved: close the PR,
     delete the branch, cut again). Then, once the full suite on the merge
     commit is green: just release-tag   (THE TAG DEPLOYS).
EOT
}

# Bump Cargo.toml + Cargo.lock, write the CHANGELOG section, delete the
# consumed change files, commit `chore(release): v<next>`.
release_commit() { # <cur> <next> <from-tag> <preamble> <computed next, when overridden>
    local cur="$1" next="$2" from="$3" pre="$4" computed="$5" sec d files=()
    # The one version line.
    sed -i.bak "/^\[workspace\.package\]/,/^\[/s/^version *= *\"$cur\"/version = \"$next\"/" Cargo.toml
    rm -f Cargo.toml.bak
    [ "$(version_at)" = "$next" ] || die "failed to rewrite [workspace.package] version"

    # Workspace members' entries in the lockfile follow; nothing else may
    # move. --offline first: a version bump needs no index.
    cargo update --workspace --offline 2>/dev/null || cargo update --workspace

    # The chart's version and appVersion: the image tag Argo pulls at the tag.
    sed -i.bak -E "s/^version: *\"?$cur\"?[[:space:]]*$/version: $next/; s/^appVersion: *\"?$cur\"?[[:space:]]*$/appVersion: \"$next\"/" "$CHART"
    rm -f "$CHART.bak"
    { grep -qx "appVersion: \"$next\"" "$CHART" && grep -qx "version: $next" "$CHART"; } \
        || die "failed to rewrite $CHART version/appVersion ($cur -> $next)"

    d="$(today)"
    sec="$(mktemp)"
    changes_section "$next" "$d" "$pre" > "$sec"
    if [ ! -s "$sec" ]; then
        # No change files at all (the first v4 release; a repo that skipped
        # them): the commit range, through git-cliff, so the release is
        # never empty.
        note "no .changes/*.md: falling back to git-cliff over ${from:+$from..}HEAD"
        cliff --tag "v$next" --strip all ${from:+"$from..HEAD"} > "$sec"
        grep -q "^## \[$next\]" "$sec" \
            || die "git-cliff produced no [$next] section: nothing to release since ${from:-the first commit}?"
    fi
    changelog_insert "$next" "$sec"
    rm -f "$sec"

    mapfile -t files < <(change_files)
    [ "${#files[@]}" = 0 ] || git rm --quiet -- "${files[@]}"

    git add Cargo.toml Cargo.lock CHANGELOG.md "$CHART"
    if [ -n "$computed" ]; then
        git commit --quiet -m "chore(release): v$next" -m "Release-Version-Override: v$next (computed v$computed)"
    else
        git commit --quiet -m "chore(release): v$next"
    fi
    note "committed: $(git log -1 --format='%h %s')"
}

# Put <version>'s section (from <file>) into CHANGELOG.md, replacing it if
# present, above every older section, which stay as released.
changelog_insert() { # <version> <section-file>
    local v="$1" sec="$2"
    awk -v want="## [$v]" -v sec="$sec" '
        function emit(   n, i, l, buf) {
            n = 0; while ((getline l < sec) > 0) buf[++n] = l
            while (n > 0 && buf[n] == "") n--          # no trailing blanks
            for (i = 1; i <= n; i++) print buf[i]
            print ""; done = 1
        }
        index($0, want) == 1 { skip = 1; if (!done) emit(); next }
        skip && /^## \[/ { skip = 0 }
        skip { next }
        /^## \[/ && !done { emit() }
        { print }
        END { if (!done) emit() }
    ' CHANGELOG.md > "$sec.new"
    mv "$sec.new" CHANGELOG.md
    grep -q "^## \[$v\]" CHANGELOG.md || die "CHANGELOG.md has no [$v] section"
}

# The branch a release/pending commit was cut from: the one whose tip is
# its parent.
release_base_of() { # <rev>
    local p b
    p="$(git rev-parse "$1^")"
    if [ "$p" = "$(git rev-parse "origin/$MAIN")" ]; then echo "$MAIN"; return 0; fi
    for b in $(git for-each-ref --format='%(refname:strip=2)' 'refs/remotes/origin/release/v*'); do
        [ "$p" = "$(git rev-parse "refs/remotes/$b")" ] && { echo "${b#origin/}"; return 0; }
    done
    return 1
}

# Push release/pending and open its PR (squash) into the branch it was cut from.
cmd_pr() {
    local ref v body base existing from
    ref="$(git rev-parse --abbrev-ref HEAD)"
    [ "$ref" = "$RELEASE_BRANCH" ] || die "run this on $RELEASE_BRANCH (on '$ref'); just release-prepare makes it"
    [ -z "$(git status --porcelain --untracked-files=no)" ] || die "working tree is dirty"
    v="$(version_at)"
    [[ "$(git log -1 --format=%s)" =~ $(release_subject_re "$v") ]] \
        || die "the head is not the release commit for v$v (just release-prepare)"
    fetch_release_refs
    base="$(release_base_of HEAD)" \
        || die "$RELEASE_BRANCH's parent is not the tip of origin/$MAIN or of a release/v* branch: it moved since the cut. Re-cut: just release-prepare"
    git push --quiet origin "HEAD:refs/heads/$ref"
    existing="$(gh pr list --repo "$REPO" --head "$ref" --base "$base" --state open --json url --jq '.[0].url' 2>/dev/null || true)"
    if [ -n "$existing" ]; then
        note "pushed; the PR is already open: $existing"; return 0
    fi
    from="$(last_tag HEAD^)"
    body="$(mktemp)"
    {
        echo "## Summary"
        echo "Release **v$v**: \`$ref\` → \`$base\`. One commit: the CalVer bump, the CHANGELOG section built from the pending \`.changes/*.md\`, and their deletion."
        echo
        echo "Merging deploys nothing. **The tag does:** after the squash merge, once the full suite on the merge commit is green, \`just release-tag\` tags \`v$v\` → deploy (server + web) → Verify live → GitHub Release + node binaries; \`task-ios\` / \`task-macos\` upload TestFlight builds marked $v."
        echo "The release guard checks this PR (one commit, version + changelog only, the computed next version, based on the current \`$base\`). If \`$base\` moves before the merge, close this PR, delete the branch and re-run \`just release-prepare\`."
        echo
        local m; m="$(migrations_between "$from" HEAD)"
        if [ -n "$m" ]; then
            echo "### ⚠ Database migrations (rollback past this release needs a data restore)"
            # shellcheck disable=SC2016,SC2086 # literal backticks; $m is a word list
            printf -- '- `%s`\n' $m
            echo
        fi
        local d; d="$(git_deps_moved "$from" HEAD)"
        if [ -n "$d" ]; then
            echo "### Git dependencies moved (check them for migrations of their own)"
            # shellcheck disable=SC2086 # $d is a word list
            printf -- '- %s\n' $d
            echo
        fi
        echo "### Changelog"
        cmd_notes "$v"
        echo
        echo "## Change files"
        echo "Consumed (deleted) by this release."
        echo
        echo "## AI disclosure"
        echo "Tool: none (scripts/release.sh). Extent: generated. Human reviewed: no."
    } > "$body"
    gh pr create --repo "$REPO" --base "$base" --head "$ref" \
        --title "chore(release): v$v" --body-file "$body"
    rm -f "$body"
}

# The commit on <branch>'s first-parent line where <version> arrived: the
# oldest of the run of commits at its tip carrying that version (the
# release's squash commit), not a later change.
release_commit_on() { # <branch> <version>
    local c target="" found_edge=0
    for c in $(git rev-list --first-parent -n 300 "origin/$1"); do
        if [ "$(version_at "$c")" = "$2" ]; then target="$c"; else found_edge=1; break; fi
    done
    [ -n "$target" ] || return 1
    [ "$found_edge" = 1 ] || die "all of $1's last 300 first-parent commits carry version $2: cannot find the release commit"
    echo "$target"
}

# The required check's latest conclusion on a commit: success, failure,
# pending, missing. A gh/network error is fatal (never "assume green").
check_state() { # <sha>
    local out
    out="$(gh api --paginate "repos/$REPO/commits/$1/check-runs?per_page=100" \
        --jq ".check_runs[] | select(.name == \"$CHECK_NAME\") | [.started_at, .status, (.conclusion // \"\")] | @tsv")" \
        || die "could not read the check runs of $1 from GitHub: refusing to tag blind"
    [ -n "$out" ] || { echo missing; return 0; }
    out="$(printf '%s\n' "$out" | sort | tail -1)"
    case "$out" in
        *$'\t'completed$'\t'success) echo success ;;
        *$'\t'completed$'\t'*) echo failure ;;
        *) echo pending ;;
    esac
}

# After the release PR squash-merged: tag v<version> on that commit (on
# main, or on a release/v* hotfix branch). The TAG is the release: it
# triggers deploy, TestFlight, node binaries and the GitHub Release.
# Pushing as the fts-forge App or a person starts the workflows; a tag
# pushed with a workflow's GITHUB_TOKEN would not.
cmd_tag() {
    local v="${1:-}" target="" branch="" b latest state branches
    fetch_release_refs
    mapfile -t branches < <(echo "$MAIN"; git for-each-ref --format='%(refname:strip=3)' 'refs/remotes/origin/release/v*')
    if [ -z "$v" ]; then
        # The untagged version on main, else on a hotfix branch.
        for b in "${branches[@]}"; do
            v="$(version_at "origin/$b")"
            git rev-parse -q --verify "refs/tags/v$v" >/dev/null || { branch="$b"; break; }
        done
        [ -n "$branch" ] || die "every branch's version is already tagged: nothing to tag"
        branches=("$branch")
    fi
    v="${v#v}"
    is_semver "$v" || die "version '$v' is not X.Y.Z"
    if git rev-parse -q --verify "refs/tags/v$v" >/dev/null; then
        die "tag v$v already exists ($(git rev-parse --short "v$v^{commit}")). Never move a tag; a bad release is superseded by the next."
    fi
    for b in "${branches[@]}"; do
        target="$(release_commit_on "$b" "$v")" && { branch="$b"; break; }
    done
    [ -n "$target" ] || die "no commit on origin/$MAIN or a release/v* branch carries version $v: has the release PR merged?"

    [[ "$(git log -1 --format=%s "$target")" =~ $(release_subject_re "$v") ]] \
        || die "$(git rev-parse --short "$target") ('$(git log -1 --format=%s "$target")') is not the release commit chore(release): v$v"
    changelog_has "$target" "$v" \
        || die "CHANGELOG.md at $(git rev-parse --short "$target") has no [$v] section: not a release commit"
    # The merge-time re-check of the release PR's base: had $branch moved
    # under the PR, the squash commit would still carry the newer change's
    # file, and that change would ship without its note.
    [ -z "$(change_files "$target")" ] \
        || die "change files are still pending at $(git rev-parse --short "$target") ($(change_files "$target" | paste -sd' ' -)): $branch moved under the release PR, so a change ships unannounced. Cut a new release instead."
    release_diff_only "$target^" "$target" \
        || die "$(git rev-parse --short "$target") changes more than a release commit may"
    latest="$(latest_tag)"
    [ -z "$latest" ] || gt "$v" "${latest#v}" || die "v$v is not greater than the latest release $latest"

    state="$(check_state "$target")"
    [ "$state" = success ] || die "'$CHECK_NAME' on $(git rev-parse --short "$target") is $state: tag only once the full suite on the release commit is green (gh run list --commit $target)"

    # Argo deploys the newest tag within minutes, so the images it will
    # pull must be on GHCR (public, built from this commit) BEFORE the tag
    # exists. images.yml pushes them when the release commit lands.
    local itag; itag="$(bash scripts/ghcr-images.sh tag "$target")"
    [ "$itag" = "$v" ] || die "$CHART at $(git rev-parse --short "$target") has appVersion $itag, not $v: the chart would deploy the wrong images"
    bash scripts/ghcr-images.sh check "$itag" "$target" \
        || die "the images for v$v are not on GHCR yet (or not public). Wait for the Images workflow on $(git rev-parse --short "$target") (gh run list -w images.yml --commit $target), then re-run."

    note "tagging v$v at $(git rev-parse --short "$target") on $branch (previous: ${latest:-none}). This deploys."
    # An annotated tag needs a committer identity; a bot checkout may have none.
    local ident=()
    git config user.email >/dev/null 2>&1 || [ -n "${GIT_COMMITTER_EMAIL:-}" ] \
        || ident=(-c "user.name=fts-forge[bot]" -c "user.email=fts-forge[bot]@users.noreply.github.com")
    git ${ident[@]+"${ident[@]}"} tag -a "v$v" "$target" -m "Task $v"
    git push --quiet origin "refs/tags/v$v"
    note "pushed v$v: deploy, task-ios, task-macos start from the tag"
    if [ "$branch" != "$MAIN" ]; then
        cmd_forward_port "$v"
    fi
}

# A hotfix while main can't ship: release/v<latest tag> (created from the
# tag if missing), and fix/<name> on it with the given commits from main
# cherry-picked (-x records where each came from), PR'd into it. The fix
# lands on main FIRST (with its change file), so it is never lost.
cmd_hotfix() {
    local name="${1:-}" latest base head c existing body
    [ -n "$name" ] && [ $# -ge 2 ] \
        || die "usage: release.sh hotfix <sta-123-slug|no-issue-slug> <commit on main>...   (fix on main first)"
    shift
    [[ "$name" =~ ^(sta-[0-9]+|gh-[0-9]+|no-issue)-[a-z0-9]+(-[a-z0-9]+)*$ ]] \
        || die "hotfix name '$name' must be <sta-N|gh-N|no-issue>-<slug> (lowercase): the branch is fix/<name>"
    [ -z "$(git status --porcelain --untracked-files=no)" ] || die "working tree is dirty"
    fetch_release_refs
    latest="$(latest_tag)"
    [ -n "$latest" ] || die "no v* tag to branch from"
    for c in "$@"; do
        git rev-parse -q --verify "$c^{commit}" >/dev/null || die "unknown commit '$c'"
        git merge-base --is-ancestor "$c" "origin/$MAIN" \
            || die "$c is not on $MAIN: land the fix on $MAIN first (with its change file), then cherry-pick it"
    done
    base="release/$latest"
    if ! git rev-parse -q --verify "refs/remotes/origin/$base" >/dev/null; then
        note "creating $base from $latest (production)"
        git push --quiet origin "refs/tags/$latest^{commit}:refs/heads/$base"
        git fetch --quiet origin "+refs/heads/$base:refs/remotes/origin/$base"
    fi
    head="fix/$name"
    git switch --quiet -c "$head" "origin/$base"
    git cherry-pick -x "$@" || die "cherry-pick conflicted: resolve, git cherry-pick --continue, then push $head and open its PR into $base"
    git push --quiet origin "HEAD:refs/heads/$head"
    existing="$(gh pr list --repo "$REPO" --head "$head" --base "$base" --state open --json url --jq '.[0].url' 2>/dev/null || true)"
    if [ -z "$existing" ]; then
        body="$(mktemp)"
        {
            echo "## Summary"
            echo "Hotfix on \`$base\` (production is \`$latest\`): cherry-picks from \`$MAIN\`."
            echo
            git log --format='- %s' "origin/$base..HEAD"
            echo
            echo "## How tested"
            echo "The full suite runs on this PR (PRs into release/*)."
            echo
            echo "## Risk and rollback"
            echo "Revert this PR on \`$base\`; production stays at \`$latest\` until the hotfix release is tagged."
            echo
            echo "## Change files"
            echo "Carried by the cherry-picked commits."
        } > "$body"
        gh pr create --repo "$REPO" --base "$base" --head "$head" \
            --title "$(git log -1 --format=%s "$1" | sed -E 's/ \(#[0-9]+\)$//')" --body-file "$body"
        rm -f "$body"
    fi
    cat >&2 <<EOT
fix/$name -> $base is open. Next:
  1. Squash-merge it once the full suite is green.
  2. just release-prepare --on $base    (release commit, next CalVer; then just release-pr, merge)
  3. just release-tag                    tags it on $base, then opens the forward-port PR into $MAIN.
EOT
}

# After a hotfix release: a PR into main carrying its CHANGELOG section,
# and deleting the change files it consumed, so main's next release does
# not list them twice. release/* is never merged back.
cmd_forward_port() {
    local v="${1:?usage: release.sh forward-port <version>}" tag sec branch consumed f existing
    v="${v#v}"; tag="v$v"
    fetch_release_refs
    git rev-parse -q --verify "refs/tags/$tag" >/dev/null || die "no tag $tag"
    if git merge-base --is-ancestor "$tag" "origin/$MAIN"; then
        note "$tag is on $MAIN already: nothing to forward-port"; return 0
    fi
    branch="chore/no-issue-forward-port-v${v//./-}"
    sec="$(mktemp)"
    { echo "## [$v] - $(git log -1 --format=%cs "$tag")"; echo
      git show "$tag:CHANGELOG.md" | awk -v want="## [$v]" '
          index($0, want) == 1 { on = 1; next }
          on && /^## \[/ { exit }
          on { print }' | sed -e '/./,$!d'; } > "$sec"
    consumed="$(git diff --name-only --diff-filter=D "$tag^" "$tag" -- .changes/)"
    [ -z "$(git status --porcelain --untracked-files=no)" ] || die "working tree is dirty"
    git switch --quiet -C "$branch" "origin/$MAIN"
    changelog_insert "$v" "$sec"
    rm -f "$sec"
    git add CHANGELOG.md
    for f in $consumed; do
        [ -e "$f" ] && git rm --quiet -- "$f"
    done
    git commit --quiet -m "chore(release): forward-port the v$v changelog"
    git push --quiet origin "HEAD:refs/heads/$branch"
    existing="$(gh pr list --repo "$REPO" --head "$branch" --base "$MAIN" --state open --json url --jq '.[0].url' 2>/dev/null || true)"
    if [ -z "$existing" ]; then
        existing="$(gh pr create --repo "$REPO" --base "$MAIN" --head "$branch" \
            --title "chore(release): forward-port the v$v changelog" \
            --body "## Summary
Hotfix release \`$tag\` shipped from \`release/\` (not \`$MAIN\`). This copies its CHANGELOG section into \`$MAIN\` and deletes the change files it consumed${consumed:+ ($(echo "$consumed" | paste -sd' ' -))}, so the next release does not list them twice.

## Change files
none needed (no crate changed)

## AI disclosure
Tool: none (scripts/release.sh). Extent: generated. Human reviewed: no." | tail -1)"
    fi
    gh pr merge "$existing" --repo "$REPO" --squash --auto 2>/dev/null \
        || note "could not enable auto-merge on $existing; merge it once green"
    note "forward-port: $existing"
}

# Is the diff base..head ONLY a release commit's: Cargo.toml's version
# line, the workspace members' version lines in Cargo.lock, the chart's
# version + appVersion lines, CHANGELOG.md and deleted .changes/*.md? (a version bump cannot change what the
# checks tested)
# true when <head>'s Cargo.lock is <base>'s with every workspace member's
# version moved from <old> to <new> (its own `version` and the
# `"name <old>"` references that disambiguate it), and nothing else:
# packages are compared as sets, so a block Cargo re-sorted still matches.
lock_is_version_bump() { # <base> <head> <old> <new>
    local dir rc
    dir="$(mktemp -d)"
    git show "$1:Cargo.lock" > "$dir/base.lock" 2>/dev/null || { rm -rf "$dir"; return 1; }
    git show "$2:Cargo.lock" > "$dir/head.lock" 2>/dev/null || { rm -rf "$dir"; return 1; }
    python3 -I - "$dir/base.lock" "$dir/head.lock" "$3" "$4" <<'PY'
import json, sys, tomllib

base_path, head_path, old, new = sys.argv[1:5]
base = tomllib.load(open(base_path, "rb"))
head = tomllib.load(open(head_path, "rb"))
# Workspace members: no `source`, at the old version.
members = {p["name"] for p in base.get("package", []) if "source" not in p and p.get("version") == old}

def bump(p):
    p = dict(p)
    if p["name"] in members and "source" not in p and p.get("version") == old:
        p["version"] = new
    if "dependencies" in p:
        p["dependencies"] = sorted(
            f"{d.split(' ', 1)[0]} {new}" if d.split(" ") == [d.split(" ", 1)[0], old] and d.split(" ", 1)[0] in members else d
            for d in p["dependencies"]
        )
    return p

def canon(pkgs):
    return sorted(json.dumps(p, sort_keys=True) for p in pkgs)

want = canon(bump(p) for p in base.get("package", []))
got = canon({**p, "dependencies": sorted(p["dependencies"])} if "dependencies" in p else p for p in head.get("package", []))
rest = lambda doc: {k: v for k, v in doc.items() if k != "package"}
sys.exit(0 if want == got and rest(base) == rest(head) else 1)
PY
    rc=$?
    rm -rf "$dir"
    return "$rc"
}

release_diff_only() { # <base> <head>
    # No `... | grep -q` pipelines: under pipefail, grep -q exits at the first
    # match and the writer dies of SIGPIPE, which reads as "no match" on a
    # big diff. Capture into a variable, then test it.
    local base="$1" head="$2" st f out vb vh
    while IFS=$'\t' read -r st f; do
        case "$f" in
            Cargo.toml|Cargo.lock|CHANGELOG.md|"$CHART") [ "$st" = M ] || return 1 ;;
            .changes/*.md) [ "$st" = D ] || return 1 ;;
            *) return 1 ;;
        esac
    done < <(git diff --no-renames --name-status "$base" "$head")
    # Cargo.toml: exactly the one [workspace.package] version line changes.
    vb="$(version_at "$base")"; vh="$(version_at "$head")"
    [ "$vb" != "$vh" ] || [ -z "$(git diff "$base" "$head" -- Cargo.toml)" ] || return 1
    out="$(git diff -U0 "$base" "$head" -- Cargo.toml | grep -E '^[+-]' | grep -vE '^(\+\+\+|---)' || true)"
    if [ -n "$out" ]; then
        [ "$(printf '%s\n' "$out" | wc -l)" -eq 2 ] || return 1
        [ "$(printf '%s\n' "$out" | grep -cE '^-version *= *"[0-9]+\.[0-9]+\.[0-9]+"[[:space:]]*$' || true)" -eq 1 ] || return 1
        [ "$(printf '%s\n' "$out" | grep -cE '^\+version *= *"[0-9]+\.[0-9]+\.[0-9]+"[[:space:]]*$' || true)" -eq 1 ] || return 1
    fi
    # Cargo.lock: only the old workspace version -> the new one. Compared by
    # MEANING, not by line: where a member shares its name with another
    # crate in the graph (inventory, notify), Cargo also rewrites the
    # references to it (` "inventory X",`), and the new version can sort the
    # member's block past the other crate's, so whole blocks move. Anything
    # else (another crate, another version, a new dependency) is not a
    # release.
    lock_is_version_bump "$base" "$head" "$vb" "$vh" || return 1
    local nb
    nb="${vh//./\\.}"
    # Chart.yaml: only its version and appVersion lines, to the new version.
    out="$(git diff -U0 "$base" "$head" -- "$CHART" | grep -E '^[+-]' | grep -vE '^(\+\+\+|---)' \
        | grep -vE "^-(version|appVersion): \"?[0-9]+\.[0-9]+\.[0-9]+\"?$|^\+version: $nb$|^\+appVersion: \"$nb\"$" || true)"
    [ -z "$out" ] || return 1
    return 0
}

# true when <rev>:CHANGELOG.md has a `## [<version>]` section. (A variable,
# not `git show | grep -q`: see release_diff_only.)
changelog_has() { # <rev> <version>
    local c; c="$(git show "$1:CHANGELOG.md" 2>/dev/null)" || return 1
    grep -q "^## \[$2\]" <<<"$c"
}

# CI (checks.yml plan, run from the BASE commit's copy of this script):
# exit 0 when this PR is only a release commit, whose content the checks
# already saw on its base.
cmd_release_only() {
    local base="${BASE_SHA:?}" head="${HEAD_SHA:?}" ref="${HEAD_REF:?}"
    [ "$ref" = "$RELEASE_BRANCH" ] || return 1
    git merge-base --is-ancestor "$base" "$head" || return 1
    [ "$(git rev-list --count "$base..$head")" = 1 ] || return 1
    [[ "$(git log -1 --format=%s "$head")" =~ $(release_subject_re "$(version_at "$head")") ]] || return 1
    [ -z "$(change_files "$head")" ] || return 1
    release_diff_only "$base" "$head"
}

# CI (release.yml, run from the BASE commit's copy of this script, never
# the PR's). Only release PRs (head release/pending) are judged; every
# other PR passes with a note.
cmd_guard() {
    local base="${BASE_SHA:?}" head="${HEAD_SHA:?}" ref="${HEAD_REF:?}" into="${BASE_REF:?}"
    local v vb latest next tip subject n ok=1
    fail() { echo "::error::$*"; ok=0; }

    if [ "$ref" != "$RELEASE_BRANCH" ]; then
        echo "::notice::not a release PR (head '$ref', not $RELEASE_BRANCH): the release guard does not apply"
        return 0
    fi
    [ "$into" = "$MAIN" ] || [[ "$into" =~ ^release/v[0-9]+\.[0-9]+\.[0-9]+$ ]] \
        || fail "a release PR goes into $MAIN or a hotfix branch release/vX.Y.Z, not '$into'"
    # Network errors FAIL: this decides with origin's tags and tip.
    git fetch --quiet --tags --force origin || { echo "::error::git fetch failed: the guard cannot see origin's tags"; exit 1; }
    tip="$(git ls-remote --exit-code --heads origin "refs/heads/$into" | cut -f1)" \
        || { echo "::error::cannot read origin/$into's tip (network, or no such branch)"; exit 1; }
    [ -n "$tip" ] || { echo "::error::origin/$into has no tip"; exit 1; }

    v="$(version_at "$head")"; vb="$(version_at "$base")"
    latest="$(latest_tag)"
    next="$(calver_next "${latest#v}")"
    is_semver "$v" || fail "Cargo.toml version '$v' is not X.Y.Z"

    [ "$base" = "$tip" ] \
        || fail "this PR's base ($(git rev-parse --short "$base")) is not the current $into ($(git rev-parse --short "$tip" 2>/dev/null || echo "${tip:0:9}")): $into moved since the cut. Close this PR, delete $RELEASE_BRANCH and re-run just release-prepare"
    if git merge-base --is-ancestor "$base" "$head"; then
        n="$(git rev-list --count "$base..$head")"
        [ "$n" = 1 ] || fail "a release PR is exactly ONE commit on top of $into (the release commit); this one has $n"
    else
        fail "$into ($(git rev-parse --short "$base")) is not in $ref: re-cut with just release-prepare"
    fi
    subject="$(git log -1 --format=%s "$head")"
    [[ "$subject" =~ $(release_subject_re "$v") ]] \
        || fail "the commit must be 'chore(release): v$v' (it is '$subject')"
    release_diff_only "$base" "$head" \
        || fail "the release commit may change only the version line in Cargo.toml, workspace version lines in Cargo.lock, CHANGELOG.md, the Chart.yaml version + appVersion lines, and delete .changes/*.md"
    [ -z "$(change_files "$head")" ] \
        || fail "change files left pending at the head ($(change_files "$head" | paste -sd' ' -)): the release consumes all of them"
    changelog_has "$head" "$v" || fail "CHANGELOG.md at the head has no [$v] section"
    git rev-parse -q --verify "refs/tags/v$v" >/dev/null && fail "tag v$v already exists"
    if [ "$v" != "$next" ]; then
        if [ -n "$latest" ] && gt "$v" "${latest#v}" \
            && git log -1 --format=%B "$head" | grep -qE "^Release-Version-Override: v${v//./\\.}( |$)"; then
            echo "::warning::v$v is not the computed next version ($next); accepted by its Release-Version-Override trailer"
        else
            fail "version $v is not the next CalVer version: expected $next (latest tag ${latest:-none}, today $(today))"
        fi
    fi

    {
        echo "### Release guard: $ref → $into"
        echo "- version: $vb → $v (computed next: $next; latest tag ${latest:-none})"
        echo "- head: \`$(git rev-parse --short "$head")\` $subject"
        echo "- base is the current $into tip: $([ "$base" = "$tip" ] && echo yes || echo NO)"
        local m; m="$(migrations_between "$(last_tag "$base")" "$head" | paste -sd, -)"
        echo "- migrations since last release: ${m:-none}"
        local d; d="$(git_deps_moved "$(last_tag "$base")" "$head" | paste -sd, -)"
        echo "- git deps moved (own migrations not listed): ${d:-none}"
        echo "- merging deploys nothing; production changes only when a v* tag is pushed"
    } >> "${GITHUB_STEP_SUMMARY:-/dev/stderr}"
    [ "$ok" = 1 ] || exit 1
    echo "release guard: ok (v$v into $into)"
}

# CI, first step of the deploy: the tag that started this run must be a
# release commit on main or on a hotfix branch release/v*. Stops a stray
# `v*` tag (on a feature branch, on a commit with another version) from
# reaching production.
cmd_tag_guard() {
    local ref="${GITHUB_REF:-}" tag v ok=1 on="" b
    fail() { echo "::error::$*"; ok=0; }
    case "$ref" in
        refs/tags/v*) tag="${ref#refs/tags/}" ;;
        *) die "this runs from a v* tag (ref is '${ref:-none}'). Production changes only by tag: just release-tag, or dispatch this workflow with the tag as its ref." ;;
    esac
    [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "$tag is not vX.Y.Z"
    v="$(version_at HEAD)"
    [ "$tag" = "v$v" ] || fail "tag $tag but Cargo.toml says $v"
    git fetch --quiet origin \
        "+refs/heads/$MAIN:refs/remotes/origin/$MAIN" \
        "+refs/heads/release/*:refs/remotes/origin/release/*" \
        || die "git fetch failed: cannot check where $tag lives"
    for b in "$MAIN" $(git for-each-ref --format='%(refname:strip=3)' 'refs/remotes/origin/release/v*'); do
        if git merge-base --is-ancestor HEAD "origin/$b"; then on="$b"; break; fi
    done
    [ -n "$on" ] || fail "$tag ($(git rev-parse --short HEAD)) is not in $MAIN's or any release/v* branch's history"
    grep -q "^## \[$v\]" CHANGELOG.md || fail "CHANGELOG.md has no [$v] section: $tag is not a release commit"
    [ "$ok" = 1 ] || exit 1
    echo "tag guard: ok ($tag at $(git rev-parse --short HEAD), on $on)"
}

# What a release carries, for its notes. The assets land after the release
# is created (node-binaries.yml, release-assets.yml, task-macos.yml), so
# this names them rather than listing what is attached at this moment.
release_contents() { # <version> <tag>
    local v="$1" tag="$2" dl="https://github.com/$REPO/releases/download/$2"
    cat <<EOF
### What this release carries
Everything Task $v ships, from this one release:

| | |
|---|---|
| **Desktop, macOS** (Apple silicon, notarized) | [\`Task-macos.dmg\`]($dl/Task-macos.dmg) — open it, drag Task to Applications |
| **Desktop, Linux** (x86_64, glibc ≥ 2.35, WebKitGTK 4.1) | [\`Task-linux-x86_64.AppImage\`]($dl/Task-linux-x86_64.AppImage) — \`chmod +x\` and run |
| **Server / headless node** | \`task-server-linux-x86_64\`, \`task-server-linux-aarch64\`, \`task-server-macos-aarch64\` + \`SHA256SUMS\` (signed: \`SHA256SUMS.minisig\`); \`curl -fsSL https://raw.githubusercontent.com/$REPO/main/scripts/install-node.sh \| sh\` |
| **Web app** | [\`task-web-$v.tar.gz\`]($dl/task-web-$v.tar.gz) — the static bundle production serves |
| **Container images** | \`ghcr.io/fasttrackstudios/task-server:$v\`, \`ghcr.io/fasttrackstudios/task-web:$v\` |

Installed desktop apps update themselves from this release: the app checks
in the background, verifies the download (sha256; on macOS also our
signature and Gatekeeper) and installs it on the next start — or now, from
the tray's *Check for updates*. On Linux that is the AppImage; the NixOS
package updates through the fleet. A running node updates with
\`task-server self-update\`.
EOF
}

# CI, on the tag after a deploy verified live (or by hand). Idempotent.
# The tag already exists (cmd_tag pushed it); this attaches the GitHub
# Release with the changelog section, migrations and moved git deps.
cmd_publish() {
    local v tag target prev notes
    v="$(version_at HEAD)"; tag="v$v"
    is_semver "$v" || die "Cargo.toml version '$v' is not X.Y.Z"
    case "${GITHUB_REF:-}" in
        refs/tags/v*) [ "${GITHUB_REF#refs/tags/}" = "$tag" ] || die "tag ${GITHUB_REF#refs/tags/} but Cargo.toml says $v" ;;
    esac
    if gh release view "$tag" --repo "$REPO" >/dev/null 2>&1; then
        note "release $tag already exists — nothing to do"; return 0
    fi
    if ! grep -q "^## \[$v\]" CHANGELOG.md; then
        note "CHANGELOG.md has no [$v] section — not a release commit, nothing to publish"; return 0
    fi
    git rev-parse -q --verify "refs/tags/$tag" >/dev/null || die "no tag $tag: tag first (just release-tag)"
    target="$(git rev-parse "$tag^{commit}")"
    prev="$(last_tag "$target^")"

    notes="$(mktemp)"
    {
        cmd_notes "$v" || true
        local m; m="$(migrations_between "$prev" "$target")"
        if [ -n "$m" ]; then
            echo
            echo "### Database migrations"
            echo "Rolling back past this release needs a data restore, not just an image swap:"
            # shellcheck disable=SC2016,SC2086 # literal backticks; $m is a word list
            printf -- '- `%s`\n' $m
        fi
        local d; d="$(git_deps_moved "$prev" "$target")"
        if [ -n "$d" ]; then
            echo
            echo "### Git dependencies moved"
            echo "Their own migrations and behaviour changes are not in the list above:"
            # shellcheck disable=SC2086 # $d is a word list
            printf -- '- %s\n' $d
        fi
        echo
        release_contents "$v" "$tag"
        echo
        echo "---"
        echo "Tag \`$tag\` at \`$(git rev-parse --short "$target")\` (images \`ghcr.io/fasttrackstudios/task-server:$v\`, \`task-web:$v\`)."
        [ -n "$prev" ] && echo "Full diff: https://github.com/$REPO/compare/$prev...$tag"
    } > "$notes"
    [ -s "$notes" ] || die "no release notes for $v"

    gh release create "$tag" --repo "$REPO" --verify-tag \
        --title "Task $v" --notes-file "$notes" --latest
    rm -f "$notes"
    note "published https://github.com/$REPO/releases/tag/$tag"
}

case "${1:-}" in
    version) shift; cmd_version "$@" ;;
    next) shift; cmd_next "$@" ;;
    notes) shift; cmd_notes "$@" ;;
    plan) shift; cmd_plan "$@" ;;
    prepare) shift; cmd_prepare "$@" ;;
    pr) shift; cmd_pr "$@" ;;
    tag) shift; cmd_tag "$@" ;;
    hotfix) shift; cmd_hotfix "$@" ;;
    forward-port) shift; cmd_forward_port "$@" ;;
    guard) shift; cmd_guard "$@" ;;
    release-only) shift; cmd_release_only "$@" ;;
    tag-guard) shift; cmd_tag_guard "$@" ;;
    publish) shift; cmd_publish "$@" ;;
    migrations) shift; migrations_between "${1:-$(last_tag)}" "${2:-HEAD}" ;;
    # Test hook: the CalVer rule alone (no git), e.g. `calver 0.2.4 2026-10-22`.
    calver) shift; calver_next "${1:-}" "${2:-$(today)}" ;;
    *) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
