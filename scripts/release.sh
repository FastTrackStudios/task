#!/usr/bin/env bash
# Task releases: ONE version, ONE tag, ONE GitHub Release. A release is a
# CalVer tag pushed on a green commit of main; nothing is committed for it.
#
# Tag-from-main (2026-10, after v26.10.1, the same flow as studio's; Task's
# specifics are in skills/release-workflow.md, the fleet `release-workflow`
# skill holds the shared rules):
#
#   feat/... fix/... --PR, squash--> main     main is the only long-lived
#       each PR carries a cargo-rail           branch. A merge to main runs
#       change file (.changes/*.md)            CI (`CI passed`) and ships
#                                              NOTHING.
#   just release [<sha>]  --> tag vYY.M.MICRO on main's newest commit whose
#       `CI passed` is green (or on <sha>)
#                                              The TAG ships, and only the
#                                              tag (deploy.yml): the images
#                                              task-server + task-web
#                                              `:X.Y.Z`, the GitHub Release
#                                              with the node binaries, the
#                                              web bundle and the Linux
#                                              desktop, TestFlight (iOS,
#                                              macOS) and the Developer-ID
#                                              macOS app. starcommand's
#                                              argocd-image-updater rolls
#                                              production to the highest
#                                              `X.Y.Z` image; this repo
#                                              deploys nothing itself.
#
#   hotfix, when main can't ship: release/v<latest tag> (from the tag)
#       <-- PR of `cherry-pick -x` of fixes already on main; once that
#       line's `CI passed` is green, `just release <sha>` tags it. Nothing
#       is forward-ported (a release commits nothing). release/* branches
#       are never merged back.
#
# Production is the latest `v*` tag.
#
# THE VERSION IS THE TAG. Releases commit no version: every release build
# runs `release.sh stamp X.Y.Z` first, which writes the tag's version into
# its own checkout's `[workspace.package] version` and Cargo.lock's
# workspace members (uncommitted), so task-server, the web bundle, the
# desktop app and the TestFlight builds (MARKETING_VER = `release.sh
# version`) all report the release. The version committed on main is
# whatever it last was (26.10.1, the last release commit): it is NOT the
# released version. scripts/check-versions.sh holds every artifact to the
# (stamped) workspace version.
#
# Versions are CalVer YY.M.MICRO (no leading zeros: Cargo, Apple and the
# cluster's image-updater filter all need that). The NEXT version is
# computed, never chosen: take the highest vX.Y.Z tag (tags with a `-`
# suffix, and `desktop-v*`, don't count); if its YY.M is this month (UTC)
# the next is MICRO+1, otherwise YY.M.0 of this month. Hotfixes use the
# same rule, so every version is unique and ascending (App Store Connect
# requires it).
#
# Release notes come from the cargo-rail change files (`cargo rail change
# add <crates> --bump <none|patch|minor|major> -m "<note>"`, one per PR)
# ADDED since the previous tag: major -> "Breaking / Migration", minor ->
# "Added", patch -> "Fixed and improved", none -> no entry. They go into
# the GitHub Release with the migrations and moved git dependencies;
# nothing is deleted or committed. A change file a hotfix line already
# shipped is not listed again. CHANGELOG.md holds the releases up to
# 26.10.1 and is no longer written.
#
#   scripts/release.sh version [REV]      the version the tree (or REV) carries
#   scripts/release.sh next               the version the next release gets
#   scripts/release.sh plan               what is live, what main holds since,
#                                          the next version, the draft notes,
#                                          migrations, moved git deps
#   scripts/release.sh release [--dry-run] [--yes] [SHA]
#                                         tag the next version on main's newest
#                                          commit with a green `CI passed` (or
#                                          on SHA: on main or release/v*, green)
#                                          and push it: THIS SHIPS
#   scripts/release.sh queued SHA         CI (release-queue.yml): did a PR merged
#                                          since SHA's last release ask for one
#                                          (label `release`)? Prints those PRs;
#                                          exit 1 when none did
#   scripts/release.sh hotfix NAME SHA... release/v<latest tag> from the tag
#                                          (if missing), fix/NAME with SHA...
#                                          cherry-picked (-x) from main, its PR
#   scripts/release.sh notes [TAG|REV]    the release notes of a tag, or the
#                                          draft for REV (default origin/main)
#   scripts/release.sh stamp X.Y.Z        CI, first step of every release build:
#                                          write X.Y.Z into this checkout's
#                                          [workspace.package] version and the
#                                          workspace members' Cargo.lock
#                                          entries (uncommitted)
#   scripts/release.sh image-tag TAG      the image tag of a release tag: the
#                                          bare CalVer (v26.10.2 -> 26.10.2);
#                                          refuses anything else
#   scripts/release.sh tag-guard          CI (every tag-triggered job, after
#                                          stamp): is this tag a release of a
#                                          green commit on main or release/v*?
#   scripts/release.sh publish [TAG]      CI: the GitHub Release for the tag
#   scripts/release.sh migrations [A] [B] migrations added in A..B
#
# `release` pushes the tag as whoever runs it: a person, or the fts-forge
# App. A tag pushed with a workflow's GITHUB_TOKEN starts no workflow, so a
# workflow must never push it with that token.
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
REPO="${GITHUB_REPOSITORY:-FastTrackStudios/task}"
MAIN_BRANCH="${MAIN_BRANCH:-main}"
CHANGES_DIR=".changes"
# The aggregate of checks.yml's full run. A release is tagged, built and
# uploaded only from a commit where it is green.
BUILD_CHECK="${BUILD_CHECK:-CI passed}"
# The PR label that asks for a release once the PR's commit is green on
# main (release-queue.yml).
RELEASE_LABEL="${RELEASE_LABEL:-release}"
# How far back `release` looks on main for a green commit.
SCAN_MAX="${RELEASE_SCAN_MAX:-60}"

die() { echo "release: $*" >&2; exit 1; }
note() { echo "release: $*" >&2; }

# `[workspace.package] version` of Cargo.toml at a revision (default: the
# working tree, which a release build has stamped).
version_at() {
    local src
    if [ -n "${1:-}" ]; then src="$(git show "$1:Cargo.toml")"; else src="$(cat Cargo.toml)"; fi
    printf '%s\n' "$src" \
        | sed -n '/^\[workspace\.package\]/,/^\[/{s/^version *= *"\([^"]*\)".*/\1/p;}' \
        | head -1
}

is_semver() { [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; }

# YY.M.MICRO: two-digit year, month 1-12 and a counter, no zero padding
# (`26.09.1` is not a valid Cargo version, and argocd-image-updater's
# filter `^[0-9]+\.[0-9]+\.[0-9]+$` with semver ordering needs exactly
# this shape to order the images).
is_calver() { [[ "$1" =~ ^[1-9][0-9]\.(1[0-2]|[1-9])\.(0|[1-9][0-9]*)$ ]]; }

# true when $1 > $2 (both X.Y.Z). sort -V, never a string compare:
# 26.10.10 > 26.10.9.
gt() { [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -1)" = "$1" ]; }

# Release tags, newest first. Only exactly vX.Y.Z: `desktop-v*` (the macOS
# app's own releases before 26.10) and any `-rc.N` suffix never match.
release_tags() { # [git tag args...]
    git -c versionsort.suffix=- tag --list 'v[0-9]*' --sort=-v:refname "$@" \
        | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' || true
}

# The highest release tag in the repo, on any line (main or a hotfix).
latest_tag() { release_tags | head -1; }

# The newest release tag reachable from a revision: the previous release
# of a commit, where its notes start.
last_tag() { release_tags --merged "${1:-HEAD}" | head -1; }

# This month as YY.M, UTC. RELEASE_MONTH=YY.M overrides it (the sandbox
# test; never set it for a real release).
this_month() {
    if [ -n "${RELEASE_MONTH:-}" ]; then
        [[ "$RELEASE_MONTH" =~ ^[1-9][0-9]\.([1-9]|1[0-2])$ ]] || die "RELEASE_MONTH '$RELEASE_MONTH' is not YY.M"
        echo "$RELEASE_MONTH"; return 0
    fi
    echo "$(date -u +%y).$((10#$(date -u +%m)))"
}

# The CalVer next-version rule (see the header). Needs the tags fetched.
next_version() {
    local tag v ym
    tag="$(latest_tag)"; ym="$(this_month)"
    v="${tag#v}"
    if [ -n "$tag" ] && [ "${v%.*}" = "$ym" ]; then
        echo "$ym.$(( ${v##*.} + 1 ))"
    else
        echo "$ym.0"
    fi
}

# Sea-orm migrations added in a range: the thing that makes a rollback a
# data restore instead of an image swap. Release notes carry it.
migrations_between() { # <from> <to>
    # Migrations here are modules inside each service's migrations.rs
    # (`mod m20261012_000001_create_activity`, registered with
    # `Box::new(m…::Migration)`, sometimes named by a string literal), so
    # look for ADDED lines carrying such a name rather than for added files.
    #
    # BLIND SPOT: migrations that arrive inside a git dependency (a
    # Cargo.lock bump of architect & co.: m20261019_000001_create_devices
    # came that way) are not in this repo's diff. The authoritative list is
    # the server's own `seaql_migrations` tables after deploy; the release
    # notes flag lockfile bumps of git deps so a human looks.
    [ -n "$1" ] || return 0
    git diff -U0 "$1" "$2" -- '*.rs' \
        | grep -E '^\+' \
        | grep -oE '\bm[0-9]{8}_[0-9]{6}_[A-Za-z0-9_]+' | sort -u || true
}

# Git dependencies whose locked commit moved in a range: where migrations
# (and behaviour) can arrive without a line of this repo's source changing.
git_deps_moved() { # <from> <to>
    [ -n "$1" ] || return 0
    git diff -U0 "$1" "$2" -- Cargo.lock \
        | grep -E '^\+source = "git\+' \
        | sed -E 's/^\+source = "git\+([^?#"]+).*/\1/' | sort -u || true
}

# `git rev-list --count --no-merges A..B`, tolerant of a missing A.
count_between() {
    if [ -n "${1:-}" ]; then git rev-list --count --no-merges "$1..$2"; else git rev-list --count --no-merges "$2"; fi 2>/dev/null || echo 0
}

# `git fetch` that FAILS loudly: a guard or a release that cannot see
# origin must never read that as "nothing there".
#
# `--force`: on a tag build, actions/checkout writes the tag as a plain
# ref to the tagged commit (`+<sha>:refs/tags/vX.Y.Z`), and `just release`
# pushes an annotated tag, so a plain `--tags` fetch refuses to "clobber"
# the local copy and fails, which this would report as origin being
# unreachable. Origin's tags are the truth here: take them.
fetch() { # <refspec>...
    git fetch --quiet --tags --force origin "$@" || die "cannot fetch from origin ($*): network or auth problem; refusing to guess"
}

# Every release line on origin: main and the hotfix branches release/v*.
fetch_lines() {
    fetch "+refs/heads/$MAIN_BRANCH:refs/remotes/origin/$MAIN_BRANCH" \
        "+refs/heads/release/v*:refs/remotes/origin/release/v*"
}

# The release line (main, or release/vX.Y.Z) whose history holds <sha>,
# main first. Empty when none does. Needs fetch_lines.
release_line_of() { # <sha>
    local b
    for b in "$MAIN_BRANCH" $(git for-each-ref --format='%(refname:lstrip=3)' 'refs/remotes/origin/release/v*'); do
        if git merge-base --is-ancestor "$1" "origin/$b" 2>/dev/null; then echo "$b"; return 0; fi
    done
    return 0
}

# Does origin have <branch>? 0 yes, 1 no; a network failure dies (ls-remote
# --exit-code says 2 for "no such ref", anything else is an error).
remote_has_branch() { # <branch>
    local rc=0
    git ls-remote --exit-code --heads origin "$1" >/dev/null 2>&1 || rc=$?
    case "$rc" in 0) return 0 ;; 2) return 1 ;; *) die "cannot reach origin to look for $1 (git ls-remote exit $rc)" ;; esac
}

# The latest run of `CI passed` on <sha> as "<status> <conclusion>" (a
# re-run supersedes a failure; "null null" when it never ran). A network or
# API failure dies.
check_state() { # <sha>
    local out
    out="$(gh api -X GET "repos/$REPO/commits/$1/check-runs" -f check_name="$BUILD_CHECK" -f per_page=100 \
        --jq '.check_runs | sort_by(.started_at) | last | "\(.status) \(.conclusion)"')" \
        || die "cannot read the '$BUILD_CHECK' check of $1 (network or auth problem); refusing to guess"
    echo "$out"
}

# `CI passed` on <sha> must have run and be green.
require_green() { # <sha>
    local out
    out="$(check_state "$1")"
    [ "$out" = "completed success" ] \
        || die "'$BUILD_CHECK' on $(git rev-parse --short "$1") is '${out:-missing}', not green: nothing ships from a red or unchecked commit (fix main, or wait for the run)"
}

# The newest first-parent commit of origin/<line> after <since> (a tag, or
# empty) whose `CI passed` is green; empty when none is. Says why it skips
# each newer one (still running, red, cancelled by a newer merge).
newest_green() { # <line> [since]
    local line="$1" since="${2:-}" c st n=0
    while read -r c; do
        [ -n "$c" ] || continue
        n=$((n + 1))
        [ "$n" -le "$SCAN_MAX" ] || { note "looked at $SCAN_MAX commits of $line: none green"; return 0; }
        st="$(check_state "$c")"
        if [ "$st" = "completed success" ]; then echo "$c"; return 0; fi
        note "skipping $(git log -1 --format='%h %s' "$c"): '$BUILD_CHECK' is '${st:-missing}'"
    done < <(if [ -n "$since" ]; then git rev-list --first-parent "origin/$line" "^$since"; else git rev-list --first-parent "origin/$line"; fi)
    return 0
}

cmd_version() { version_at "$@"; } # [rev]
cmd_next() {
    git fetch --quiet --tags --force origin 2>/dev/null || note "offline: next version from the local tags"
    next_version
}

# ── change files (.changes/*.md, written by `cargo rail change add`) ──────
#
#   ---
#   "task-server" = "patch"
#   "files-store" = "minor"
#   ---
#
#   Fixed sync stalling after offline edits
#
#   Migration: …        (major only)
#
# A file's bump is the highest of its crates'. .changes/README is not a
# change file (and is not `.md`: rail rejects any .md without the front
# matter).

# The change files a release of <sha> lists, oldest first (the order their
# PRs merged): those ADDED between <prev> (the previous tag; empty for the
# whole history) and <sha>, minus the ones a hotfix release on another line
# already shipped (a newer release tag not merged into <sha> whose tree has
# the file: it was cherry-picked there with its fix).
release_change_files() { # <prev> <sha> [below]
    local prev="$1" sha="$2" below="${3:-}" added shipped="" t
    if [ -n "$prev" ]; then
        added="$(git diff --no-renames --diff-filter=A --name-only "$prev" "$sha" -- "$CHANGES_DIR/" | grep -E '\.md$' || true)"
    else
        added="$(git ls-tree --name-only "$sha" "$CHANGES_DIR/" | grep -E '\.md$' || true)"
    fi
    [ -n "$added" ] || return 0
    for t in $(other_line_tags "$sha" "$below"); do
        [ -z "$prev" ] || gt "${t#v}" "${prev#v}" || continue
        shipped+="$(git ls-tree --name-only "$t" "$CHANGES_DIR/" | grep -E '\.md$' || true)"$'\n'
    done
    shipped="$(grep . <<<"$shipped" || true)"
    if [ -n "$shipped" ]; then
        added="$(grep -vxF -f <(printf '%s\n' "$shipped") <<<"$added" || true)"
        [ -n "$added" ] || return 0
    fi
    { if [ -n "$prev" ]; then git log --diff-filter=A --reverse --format= --name-only "$prev..$sha" -- "$CHANGES_DIR" 2>/dev/null || true
      else git log --diff-filter=A --reverse --format= --name-only "$sha" -- "$CHANGES_DIR" 2>/dev/null || true; fi
      printf '%s\n' "$added"; } \
        | awk 'NF && !seen[$0]++' | grep -Fx -f <(printf '%s\n' "$added") || true
}

# major | minor | patch | none, or "bad" for a file that is not one.
change_bump() { # <rev> <file>
    git show "$1:$2" | awk '
        NR == 1 { if ($0 != "---") { bad = 1; exit } next }
        $0 == "---" { closed = 1; exit }
        /^[[:space:]]*(#|$)/ { next }
        {
            v = $0; sub(/^[^=]*=[[:space:]]*"/, "", v); sub(/".*$/, "", v)
            r = (v == "major") ? 3 : (v == "minor") ? 2 : (v == "patch") ? 1 : (v == "none") ? 0 : -1
            if (r < 0) bad = 1
            if (r > max) max = r
            n++
        }
        END {
            if (bad || !closed || n == 0) { print "bad"; exit }
            split("none patch minor major", name, " "); print name[max + 1]
        }'
}

# The note: everything after the front matter, as one list item (later
# lines and paragraphs indented under it, so a Migration: paragraph stays
# with its entry).
change_entry() { # <rev> <file>
    git show "$1:$2" \
        | awk 'c >= 2 { print; next } $0 == "---" { c++ }' \
        | sed -e '/./,$!d' \
        | awk '{ l[NR] = $0 } END { n = NR; while (n > 0 && l[n] ~ /^[[:space:]]*$/) n--
                 for (i = 1; i <= n; i++) {
                   if (i == 1) print "- " l[i]; else if (l[i] == "") print ""; else print "  " l[i] } }'
}

# Release tags on ANOTHER line than <sha>: neither in its history nor
# after it (a hotfix tag on a release/v* line, seen from main), newest
# first. With <below> (the version being described), only the ones older
# than it: notes of a tag never change once later releases exist.
other_line_tags() { # <sha> [below]
    local t
    for t in $(release_tags --no-merged "$1" --no-contains "$1"); do
        [ -z "${2:-}" ] || gt "${2#v}" "${t#v}" || continue
        echo "$t"
    done
}

# Hotfix releases tagged on a release/v* line after <prev>: their fixes
# reached main first, so main's next notes say they cover them.
includes_line() { # <prev> <sha> [below]
    local hot
    hot="$(other_line_tags "$2" "${3:-}" | head -1)"
    [ -n "$hot" ] || return 0
    if [ -z "$1" ] || gt "${hot#v}" "${1#v}"; then
        echo "Includes all changes up to $hot."
    fi
}

# The release notes of <sha> since <prev>, grouped the way the CHANGELOG
# sections were (Breaking / Migration, Added, Fixed and improved; `none`
# lists nothing). Read from <sha>'s tree. <version>: the release they
# describe (empty for a draft).
render_notes() { # <prev> <sha> [version]
    local prev="$1" sha="$2" ver="${3:-}" files f b group title any=0 out incl
    files="$(release_change_files "$prev" "$sha" "$ver")"
    if [ -n "$files" ]; then
        while read -r f; do
            b="$(change_bump "$sha" "$f")"
            [ "$b" != bad ] || die "$f is not a change file (front matter: \"crate\" = \"none|patch|minor|major\"): fix it on main (cargo rail change add)"
        done <<<"$files"
    fi
    incl="$(includes_line "$prev" "$sha" "$ver")"
    if [ -n "$incl" ]; then echo "$incl"; echo; fi
    for group in major minor patch; do
        [ -n "$files" ] || break
        case "$group" in
            major) title="Breaking / Migration" ;;
            minor) title="Added" ;;
            patch) title="Fixed and improved" ;;
        esac
        out=""
        while read -r f; do
            [ "$(change_bump "$sha" "$f")" = "$group" ] || continue
            out+="$(change_entry "$sha" "$f")"$'\n'
        done <<<"$files"
        [ -n "$out" ] || continue
        any=1
        echo "### $title"
        echo
        printf '%s\n' "$out"
    done
    if [ "$any" = 0 ]; then
        echo "No user-visible changes (internal changes only)."
        echo
    fi
}

# The migrations and moved git deps of <prev>..<sha>, as notes sections
# (nothing when there are none).
render_risks() { # <prev> <sha>
    local m d
    m="$(migrations_between "$1" "$2")"
    if [ -n "$m" ]; then
        echo "### Database migrations"
        echo "Rolling back past this release needs a data restore, not just an image swap:"
        # shellcheck disable=SC2016,SC2086 # literal backticks; $m is a word list
        printf -- '- `%s`\n' $m
        echo
    fi
    d="$(git_deps_moved "$1" "$2")"
    if [ -n "$d" ]; then
        echo "### Git dependencies moved"
        echo "Their own migrations and behaviour changes are not in the list above:"
        # shellcheck disable=SC2086 # $d is a word list
        printf -- '- %s\n' $d
        echo
    fi
}

# The notes of a tag (since the tag before it), or the draft for a
# revision (default origin/main) since its newest tag.
cmd_notes() { # [TAG|REV]
    local r="${1:-origin/$MAIN_BRANCH}" sha prev
    sha="$(git rev-parse -q --verify "$r^{commit}")" || die "'$r' is not a tag or commit"
    if git rev-parse -q --verify "refs/tags/$r" >/dev/null && [[ "$r" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        prev="$(last_tag "$sha^")"
        render_notes "$prev" "$sha" "$r"
    else
        prev="$(last_tag "$sha")"
        render_notes "$prev" "$sha"
    fi
    render_risks "$prev" "$sha"
}

cmd_plan() {
    git fetch --quiet --tags --force origin "+refs/heads/$MAIN_BRANCH:refs/remotes/origin/$MAIN_BRANCH" 2>/dev/null \
        || note "offline: planning from the local origin/$MAIN_BRANCH and tags"
    local ref="origin/$MAIN_BRANCH" tag mtag next
    tag="$(latest_tag)"; mtag="$(last_tag "$ref")"; next="$(next_version)"
    echo "production (latest v* tag):  ${tag:-none}"
    echo "on $MAIN_BRANCH since ${mtag:-the start}:     $(count_between "$mtag" "$ref") commits"
    echo "change files since ${mtag:-the start}:  $(release_change_files "$mtag" "$ref" | grep -c . || true)"
    echo "next release:                v$next (CalVer YY.M.MICRO from the highest tag and this month)"
    echo "                             just release tags it on $MAIN_BRANCH's newest commit with a green '$BUILD_CHECK'"
    echo "                             (just release --dry-run names that commit)"
    echo "committed version:           $(version_at "$ref") (not the released one: builds stamp the tag's)"
    echo
    echo "── draft notes ($ref) ──"
    render_notes "$mtag" "$ref"
    if [ -n "$mtag" ]; then
        local m d
        m="$(migrations_between "$mtag" "$ref")"
        echo "── migrations since $mtag ──"
        echo "${m:-none}"
        d="$(git_deps_moved "$mtag" "$ref")"
        echo "── git dependencies moved since $mtag (their migrations are not listed above) ──"
        echo "${d:-none}"
    fi
}

# Tag the next version on a green commit and push it. THE TAG SHIPS: the
# images, the Release and its assets, TestFlight; nothing here waits for
# any of them.
cmd_release() { # [--dry-run] [--yes] [SHA]
    local dry=0 yes=0 want="" sha line tag prev next ltag existing a
    while [ $# -gt 0 ]; do
        case "$1" in
            --dry-run|-n) dry=1 ;;
            --yes|-y) yes=1 ;;
            -*) die "unknown option '$1' (release [--dry-run] [--yes] [SHA])" ;;
            *) [ -z "$want" ] || die "one commit at most"; want="$1" ;;
        esac
        shift
    done
    fetch_lines
    tag="$(latest_tag)"
    if [ -n "$want" ]; then
        sha="$(git rev-parse -q --verify "$want^{commit}")" || die "'$want' is not a commit (fetched origin's main and release/v*)"
        line="$(release_line_of "$sha")"
        [ -n "$line" ] || die "$(git rev-parse --short "$sha") is on neither origin/$MAIN_BRANCH nor an origin/release/v* line: only a commit that reached one is released"
        require_green "$sha"
    else
        line="$MAIN_BRANCH"
        ltag="$(last_tag "origin/$line")"
        sha="$(newest_green "$line" "$ltag")"
        [ -n "$sha" ] || die "no commit on $MAIN_BRANCH since ${ltag:-the start} has a green '$BUILD_CHECK' (gh run list -b $MAIN_BRANCH -w checks.yml): wait for the run, or fix main"
    fi
    existing="$(git tag --points-at "$sha" | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1 || true)"
    [ -z "$existing" ] || die "$(git rev-parse --short "$sha") is already released as $existing: nothing new to ship"
    # Never a commit older than its line's newest release: that would ship
    # a regression under a newer version.
    ltag="$(last_tag "origin/$line")"
    if [ -n "$ltag" ] && git merge-base --is-ancestor "$sha" "$ltag"; then
        die "$(git rev-parse --short "$sha") is older than $ltag on $line: a release only moves $line forward"
    fi
    prev="$(last_tag "$sha")"
    next="$(next_version)"
    is_calver "$next" || die "next version '$next' is not CalVer YY.M.MICRO"
    if [ -n "$tag" ]; then
        gt "$next" "${tag#v}" || die "the next version $next is not above the highest tag $tag (the clock, or RELEASE_MONTH?): versions only increase"
    fi
    git rev-parse -q --verify "refs/tags/v$next" >/dev/null && die "tag v$next already exists"

    echo "release v$next: $(git log -1 --format='%h %s' "$sha")"
    echo "  on $line, $(count_between "$prev" "$sha") commits since ${prev:-the start}; highest tag ${tag:-none}; '$BUILD_CHECK' green"
    echo
    echo "── release notes ──"
    render_notes "$prev" "$sha" "$next"
    render_risks "$prev" "$sha"
    if [ "$dry" = 1 ]; then note "dry run: v$next not tagged"; return 0; fi
    if [ "$yes" = 0 ] && [ -t 0 ]; then
        printf 'Push v%s? It ships the images (production follows), the Release and TestFlight. [y/N] ' "$next" >&2
        read -r a
        [ "$a" = y ] || [ "$a" = Y ] || die "not tagged"
    fi
    # An annotated tag needs a committer identity; a bot checkout may have none.
    local ident=()
    git config user.email >/dev/null 2>&1 || [ -n "${GIT_COMMITTER_EMAIL:-}" ] \
        || ident=(-c "user.name=fts-forge[bot]" -c "user.email=fts-forge[bot]@users.noreply.github.com")
    git ${ident[@]+"${ident[@]}"} tag -a "v$next" "$sha" -m "Task $next"
    git push --quiet origin "refs/tags/v$next"
    note "pushed v$next at $(git rev-parse --short "$sha") on $line: Deploy, task-ios and task-macos start from the tag"
}

# Did a PR merged since <sha>'s last release ask for one? A PR asks with
# the `release` label; its squash commit, or any commit after it up to
# <sha>, then ships once green — so a labelled PR whose own run went red is
# still released by the next green commit, and one released already is not
# asked again. Prints the asking PRs (`#348 #351`); exit 1 when none.
cmd_queued() { # <sha>
    local want="${1:-}" sha prev c asked=()
    [ -n "$want" ] || die "queued needs a commit"
    sha="$(git rev-parse -q --verify "$want^{commit}")" || die "'$want' is not a commit"
    prev="$(last_tag "$sha")"
    for c in $(git rev-list --first-parent "${prev:+$prev..}$sha"); do
        while IFS= read -r pr; do
            [ -n "$pr" ] && asked+=("#$pr")
        done < <(gh api "repos/$REPO/commits/$c/pulls"             --jq ".[] | select(any(.labels[]; .name == \"$RELEASE_LABEL\")) | .number" 2>/dev/null || true)
    done
    [ ${#asked[@]} -gt 0 ] || return 1
    echo "${asked[*]}"
}

# Write <version> into the working tree's [workspace.package] version and
# the workspace members' entries in Cargo.lock: the release build's own
# checkout, never committed. Every crate with `version.workspace = true`
# then builds as the release, so CARGO_PKG_VERSION, the TestFlight
# marketing version, the desktop app's version and the update check all
# agree with the tag. A member is a [[package]] with no `source` line whose
# version is the old workspace version. Where a member shares its name with
# another crate in the graph (inventory, notify), Cargo names it in
# dependency lists as `"inventory <version>"`; those references move too,
# or `cargo --locked` refuses the lockfile. Nothing else in the lockfile
# moves (no cargo re-lock, which would re-resolve registry picks).
# Idempotent.
cmd_stamp() { # <X.Y.Z | vX.Y.Z>
    local v="${1:-}" cur lock
    v="${v#v}"
    is_semver "$v" || die "usage: release.sh stamp X.Y.Z (got '${1:-}')"
    cur="$(version_at)"
    [ -n "$cur" ] || die "Cargo.toml has no [workspace.package] version"
    if [ "$cur" = "$v" ]; then
        note "stamp: the checkout is already $v"
        return 0
    fi
    sed "/^\[workspace\.package\]/,/^\[/s/^version *= *\"$cur\"/version = \"$v\"/" Cargo.toml > Cargo.toml.stamp \
        && mv Cargo.toml.stamp Cargo.toml
    [ "$(version_at)" = "$v" ] || die "stamp: failed to rewrite [workspace.package] version"
    lock="$(mktemp)"
    # Two passes over Cargo.lock: the first collects the members' names, the
    # second rewrites their versions and the `"<member> <old>"` references.
    awk -v cur="$cur" -v nv="$v" '
        function header(l) { return l ~ /^\[\[package\]\]$/ || l ~ /^\[\[patch\.unused\]\]$/ || l ~ /^\[metadata\]$/ }
        function note_member() { if (pkg && name != "" && !src && ver == cur) member[name] = 1 }
        function flush(   i, ref, sp) {
            for (i = 1; i <= n; i++) {
                if (pkg && !src && buf[i] == "version = \"" cur "\"") buf[i] = "version = \"" nv "\""
                else if (buf[i] ~ /^ "[^" ]+ [^" ]+",?$/) {
                    ref = buf[i]; sub(/^ "/, "", ref); sub(/",?$/, "", ref); sp = index(ref, " ")
                    if ((substr(ref, 1, sp - 1) in member) && substr(ref, sp + 1) == cur) sub(cur "\"", nv "\"", buf[i])
                }
                print buf[i]
            }
            n = 0; src = 0; pkg = 0
        }
        FNR == NR {
            if (header($0)) { note_member(); pkg = ($0 == "[[package]]"); name = ""; ver = ""; src = 0; next }
            if ($0 ~ /^name = "/) { name = $0; sub(/^name = "/, "", name); sub(/"$/, "", name) }
            else if ($0 ~ /^version = "/) { ver = $0; sub(/^version = "/, "", ver); sub(/"$/, "", ver) }
            else if ($0 ~ /^source = /) src = 1
            next
        }
        FNR == 1 { note_member(); n = 0; src = 0; pkg = 0 }
        header($0) { flush(); pkg = ($0 == "[[package]]") }
        { buf[++n] = $0; if ($0 ~ /^source = /) src = 1 }
        END { flush() }
    ' Cargo.lock Cargo.lock > "$lock" && cat "$lock" > Cargo.lock
    rm -f "$lock"
    note "stamp: $cur -> $v in Cargo.toml and Cargo.lock (uncommitted: the committed version is not the released one)"
}

# The image tag of a release tag: the bare CalVer, which is what
# argocd-image-updater orders (semver over `^[0-9]+\.[0-9]+\.[0-9]+$`).
# Anything else (a `v`, a leading zero, a suffix) is refused: it would
# sort wrong or be ignored by the cluster.
cmd_image_tag() { # <tag>
    local t="${1:-}" v
    v="${t#v}"
    [ "$t" != "$v" ] || die "'$t' is not a release tag (vYY.M.MICRO)"
    is_calver "$v" || die "'$t' is not vYY.M.MICRO CalVer (no leading zeros, month 1-12): its image tag would not match the cluster's ^[1-9][0-9]\\.(1[0-2]|[1-9])\\.(0|[1-9][0-9]*)\$"
    echo "$v"
}

# A hotfix while main can't ship: release/v<latest tag> (created from the
# tag if missing), and fix/<name> on it with the given commits from main
# cherry-picked (-x records where each came from), PR'd into it. The fix
# lands on main FIRST (with its change file), so it is never lost. Once it
# merges and that line's `CI passed` is green: `just release <merge sha>`.
cmd_hotfix() {
    local name="${1:-}" latest line head c existing body
    [ -n "$name" ] && [ $# -ge 2 ] \
        || die "usage: release.sh hotfix <sta-123-slug|no-issue-slug> <commit on main>...   (fix on main first)"
    shift
    [[ "$name" =~ ^(sta-[0-9]+|gh-[0-9]+|no-issue)-[a-z0-9]+(-[a-z0-9]+)*$ ]] \
        || die "hotfix name '$name' must be <sta-N|gh-N|no-issue>-<slug> (lowercase): the branch is fix/<name>"
    [ -z "$(git status --porcelain --untracked-files=no)" ] || die "working tree is dirty"
    fetch "+refs/heads/$MAIN_BRANCH:refs/remotes/origin/$MAIN_BRANCH"
    latest="$(latest_tag)"
    [ -n "$latest" ] || die "no release tag yet: nothing to hotfix"
    for c in "$@"; do
        git rev-parse -q --verify "$c^{commit}" >/dev/null || die "'$c' is not a commit"
        git merge-base --is-ancestor "$c" "origin/$MAIN_BRANCH" \
            || die "$c is not on origin/$MAIN_BRANCH: a fix lands on main first (with its change file), then is picked"
    done
    line="release/$latest"
    if ! remote_has_branch "$line"; then
        git push --quiet origin "$(git rev-parse "$latest^{commit}"):refs/heads/$line"
        note "created $line at $latest (production)"
    fi
    fetch "+refs/heads/$line:refs/remotes/origin/$line"
    head="fix/$name"
    git switch --quiet -C "$head" "origin/$line"
    for c in "$@"; do
        git cherry-pick -x "$c" >/dev/null \
            || die "cherry-pick of $c conflicts: resolve it, git cherry-pick --continue, pick the rest, push $head and open its PR into $line"
    done
    git push --quiet -u origin "HEAD:refs/heads/$head"
    existing="$(gh pr list --repo "$REPO" --head "$head" --base "$line" --state open --json url --jq '.[0].url' 2>/dev/null || true)"
    if [ -z "$existing" ]; then
        body="$(mktemp)"
        {
            echo "## Summary"
            echo "Hotfix onto \`$line\` (production is \`$latest\`). Cherry-picked with \`-x\` from \`$MAIN_BRANCH\`, where each fix landed first:"
            echo
            git log --format='- %s' "origin/$line..HEAD"
            echo
            echo "The full suite runs on this PR, and again on \`$line\` once it merges. Then \`just release <merge sha>\` tags the next version on it (once its \`$BUILD_CHECK\` is green). Nothing is forward-ported: a release commits nothing."
            echo
            echo "## Issue"
            echo "Refs: see the picked commits"
            echo
            printf '## How tested\n%s\n\n## Risk and rollback\n%s\n\n## Change files\n%s\n\n## AI disclosure\nTool: none (scripts/release.sh). Extent: generated. Human reviewed: no.\n' \
                "The picked commits passed checks on $MAIN_BRANCH; the full suite runs here." \
                "Revert the merge on \`$line\` before it is tagged; after, release the next version." \
                "the picked commits' own change files"
        } > "$body"
        gh pr create --repo "$REPO" --base "$line" --head "$head" \
            --title "$(git log -1 --format=%s "$1" | sed -E 's/ \(#[0-9]+\)$//')" --body-file "$body"
        rm -f "$body"
    fi
    note "$head -> $line is open. Once it merges and $line's '$BUILD_CHECK' is green: just release <merge sha>"
}

# Kept for older workflow copies (a release/v* line cut before tag-from-main
# runs its BASE commit's copy of this script): no PR is a release PR.
cmd_release_only() { return 1; }

# CI, in every tag-triggered job, AFTER `stamp`: the tag that started this
# run must be vX.Y.Z on a commit of main or of a release/v* hotfix line,
# with `CI passed` green on it (with GH_TOKEN set). The test suite is not
# run again: that green check is the proof. Stops a stray `v*` tag (on a
# feature branch, on a red commit) from reaching production.
cmd_tag_guard() {
    local ref="${GITHUB_REF:-}" tag v ok=1 line
    fail() { echo "::error::$*"; ok=0; }
    case "$ref" in
        refs/tags/v*) tag="${ref#refs/tags/}" ;;
        *) die "this runs from a v* tag (ref is '${ref:-none}'). Production changes only by tag: just release, or dispatch this workflow with the tag as its ref." ;;
    esac
    [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "tag $tag is not vX.Y.Z"
    v="$(version_at)"
    [ "$tag" = "v$v" ] || fail "tag $tag but this checkout's version is $v: run 'release.sh stamp $tag' first"
    if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
        [ "$(git rev-parse "$tag^{commit}")" = "$(git rev-parse HEAD)" ] || fail "this checkout ($(git rev-parse --short HEAD)) is not $tag"
    fi
    fetch_lines
    line="$(release_line_of HEAD)"
    [ -n "$line" ] || fail "$tag ($(git rev-parse --short HEAD)) is on neither $MAIN_BRANCH nor a release/v* line: releases are tagged there"
    [ "$ok" = 1 ] || exit 1
    if [ -n "${GH_TOKEN:-}" ]; then require_green "$(git rev-parse HEAD)"; else note "no GH_TOKEN: '$BUILD_CHECK' not checked here (just release checked it)"; fi
    echo "tag guard: ok ($tag at $(git rev-parse --short HEAD), on $line)"
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

# CI, on the tag (and by hand). The tag already exists (`release` pushed
# it); this attaches the GitHub Release: the notes from the change files
# added since the previous tag, migrations, moved git deps and what the
# release carries. Idempotent. With no argument the tag is the ref of the
# run.
cmd_publish() { # [tag]
    local v tag="${1:-}" target prev notes line
    if [ -z "$tag" ]; then
        case "${GITHUB_REF:-}" in refs/tags/v*) tag="${GITHUB_REF#refs/tags/}" ;; esac
    fi
    [ -n "$tag" ] || die "publish needs a tag (an argument, or a run on refs/tags/v*)"
    v="${tag#v}"
    is_semver "$v" || die "version '$v' is not X.Y.Z"
    if gh release view "$tag" --repo "$REPO" >/dev/null 2>&1; then
        note "release $tag already exists — nothing to do"; return 0
    fi
    git rev-parse -q --verify "refs/tags/$tag" >/dev/null || die "no tag $tag: tag first (just release)"
    target="$(git rev-parse "$tag^{commit}")"
    prev="$(last_tag "$target^")"
    line="$(release_line_of "$target")"

    notes="$(mktemp)"
    {
        render_notes "$prev" "$target" "$v"
        render_risks "$prev" "$target"
        release_contents "$v" "$tag"
        echo
        echo "---"
        echo "Tag \`$tag\` at \`$(git rev-parse --short "$target")\` on ${line:-$MAIN_BRANCH} (images \`ghcr.io/fasttrackstudios/task-server:$v\`, \`task-web:$v\`)."
        [ -n "$prev" ] && echo "Full diff: https://github.com/$REPO/compare/$prev...$tag"
    } > "$notes"
    [ -s "$notes" ] || die "no release notes for $v"

    if ! gh release create "$tag" --repo "$REPO" --verify-tag \
        --title "Task $v" --notes-file "$notes" --latest; then
        gh release view "$tag" --repo "$REPO" >/dev/null 2>&1 \
            || die "could not create the GitHub Release for $tag"
        note "release $tag was created meanwhile — nothing to do"
    fi
    rm -f "$notes"
    note "published https://github.com/$REPO/releases/tag/$tag"
}

case "${1:-}" in
    version) shift; cmd_version "$@" ;;
    next) shift; cmd_next "$@" ;;
    notes) shift; cmd_notes "$@" ;;
    plan) shift; cmd_plan "$@" ;;
    release) shift; cmd_release "$@" ;;
    queued) shift; cmd_queued "$@" ;;
    hotfix) shift; cmd_hotfix "$@" ;;
    stamp) shift; cmd_stamp "$@" ;;
    image-tag) shift; cmd_image_tag "$@" ;;
    release-only) shift; cmd_release_only "$@" ;;
    tag-guard) shift; cmd_tag_guard "$@" ;;
    publish) shift; cmd_publish "$@" ;;
    migrations) shift; migrations_between "${1:-$(last_tag)}" "${2:-HEAD}" ;;
    *) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
