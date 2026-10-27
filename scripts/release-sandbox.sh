#!/usr/bin/env bash
# Sandbox test of scripts/release.sh (tag-from-main): CalVer next versions,
# the image tag rule, `release` picking main's newest green commit and
# refusing what must not ship, the notes from the change files added since
# the previous tag (with migrations and what the release carries), `stamp`,
# the tag guard, `publish` and a hotfix line, end to end on a synthetic repo
# and a local bare "origin". Nothing leaves the sandbox: gh is a shim (it
# keeps releases and PRs as files, and answers `CI passed` from a list of
# green commits). Run from anywhere (`just release-sandbox`):
#
#   scripts/release-sandbox.sh [SCRATCH_DIR]
#
# Needs git and awk. Exits non-zero if any check fails. Edit files in place
# with `sedi`, never `sed -i`, whose argument BSD sed reads as a backup
# suffix (release.sh runs on the Macs too).
# shellcheck disable=SC2015,SC2016 # `[ test ] && ok || bad` is the idiom here: ok never fails; the ERE patterns hold literal backticks
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# A git hook (prek's pre-push `local-ci`) exports GIT_DIR, GIT_INDEX_FILE
# and friends; left set, every git call below hits the pushing repo instead
# of the sandbox (STA-521: it set core.bare, origin's URL and tags there).
# shellcheck disable=SC2046 # one variable name per word
unset $(git rev-parse --local-env-vars)
S="${1:-${TMPDIR:-/tmp}/task-release-sandbox}"
REPO=FastTrackStudios/task
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=t@example.invalid
export GIT_CONFIG_GLOBAL=/dev/null
rm -rf "$S"; mkdir -p "$S/shim" "$S/releases" "$S/prs"
export PATH="$S/shim:$PATH"
export GITHUB_REPOSITORY="$REPO"
unset GITHUB_STEP_SUMMARY GITHUB_OUTPUT GH_TOKEN RELEASE_MONTH SHIM_CHECK GITHUB_REF
pass=0; failn=0
ok()  { echo "PASS: $*"; pass=$((pass+1)); }
bad() { echo "FAIL: $*"; failn=$((failn+1)); }
# Every command runs with stdin from /dev/null: `release` asks before it
# pushes only on a terminal.
expect_ok()   { local d="$1"; shift; if "$@" >"$S/last.out" 2>&1 </dev/null; then ok "$d"; else bad "$d"; cat "$S/last.out"; fi; }
expect_fail() { local d="$1"; shift; if "$@" >"$S/last.out" 2>&1 </dev/null; then bad "$d (unexpected pass)"; cat "$S/last.out"; else ok "$d"; grep -h '::error::\|release:' "$S/last.out" | head -2 | sed 's/^/      /'; fi; }
expect_rc()   { local d="$1" want="$2"; shift 2; "$@" >"$S/last.out" 2>&1 </dev/null; local rc=$?; [ "$rc" = "$want" ] && ok "$d (exit $rc)" || { bad "$d (exit $rc, wanted $want)"; cat "$S/last.out"; }; }
expect_out()  { if grep -qiE "$3" "$2"; then ok "$1"; else bad "$1 (no match for: $3)"; sed 's/^/      | /' "$2" | head -30; fi; }
expect_no()   { if grep -qiE "$3" "$2"; then bad "$1 (found: $3)"; else ok "$1"; fi; }
sedi()        { local e="$1" f; shift; for f in "$@"; do sed "$e" "$f" > "$f.sedi" && mv "$f.sedi" "$f"; done; }
eq()          { [ "$2" = "$3" ] && ok "$1: $2" || bad "$1: got '$2', wanted '$3'"; }
lastpr()      { local f n=0; for f in "$S"/prs/*.meta; do [ -e "$f" ] && n=$((n+1)); done; echo "$n"; } # the newest PR's number

# ── the gh shim ──────────────────────────────────────────────────────────
# `CI passed` of a commit: $SHIM_CHECK if set (every commit), else green
# when the commit is listed in $SHIM_GREEN, else $SHIM_OTHER (running).
cat > "$S/shim/gh" <<'EOF'
#!/usr/bin/env bash
# A tiny gh: releases and PRs are files, calls are logged to $SHIM_LOG.
echo "[shim] gh $*" >> "$SHIM_LOG"
arg() { local k="$1"; shift; while [ $# -gt 0 ]; do [ "$1" = "$k" ] && { echo "$2"; return; }; shift; done; }
case "$1 $2" in
  "release view") [ -f "$SHIM_REL/$3.body" ] || exit 1 ;;
  "release create") tag="$3"; shift 3
     cp "$(arg --notes-file "$@")" "$SHIM_REL/$tag.body"; arg --title "$@" > "$SHIM_REL/$tag.title" ;;
  "pr list") exit 0 ;;
  "pr create") shift 2; n=$(( $(ls "$SHIM_PRS" | grep -c meta) + 1 ))
     { echo "base=$(arg --base "$@")"; echo "head=$(arg --head "$@")"; echo "title=$(arg --title "$@")"; } > "$SHIM_PRS/$n.meta"
     f="$(arg --body-file "$@")"; if [ -n "$f" ]; then cp "$f" "$SHIM_PRS/$n.body"; else arg --body "$@" > "$SHIM_PRS/$n.body"; fi
     echo "https://github.com/x/y/pull/$n" ;;
  "api -X") case "$*" in *check-runs*)
       sha="$(printf '%s\n' "$*" | sed -n 's#.*commits/\([0-9a-f]*\)/check-runs.*#\1#p')"
       if [ -n "${SHIM_CHECK:-}" ]; then echo "$SHIM_CHECK"
       elif grep -qx "$sha" "$SHIM_GREEN" 2>/dev/null; then echo "completed success"
       else echo "${SHIM_OTHER:-in_progress null}"; fi ;; esac ;;
esac
EOF
chmod +x "$S/shim/gh"
export SHIM_LOG="$S/gh.log" SHIM_REL="$S/releases" SHIM_PRS="$S/prs" SHIM_GREEN="$S/green"; : > "$SHIM_LOG"; : > "$SHIM_GREEN"
green() { local r; for r in "$@"; do git rev-parse "$r^{commit}" >> "$SHIM_GREEN"; done; } # mark commits' CI passed green

# ── a synthetic repo: the real script ────────────────────────────────────
W="$S/seed"; mkdir -p "$W/scripts" "$W/apps/server/src" "$W/apps/desktop" "$W/features/files-store" "$W/.changes"
cp "$HERE/scripts/release.sh" "$W/scripts/"
printf 'Change files: cargo rail change add <crates> --bump <none|patch|minor|major> -m "<note>"\n' > "$W/.changes/README"
printf '[workspace]\nmembers = ["apps/server", "apps/desktop", "features/files-store"]\n\n[workspace.package]\nversion = "26.9.4"\nedition = "2021"\n' > "$W/Cargo.toml"
printf '[package]\nname = "task-server"\nversion.workspace = true\n' > "$W/apps/server/Cargo.toml"
printf '[package]\nname = "task-app-desktop"\nversion.workspace = true\n' > "$W/apps/desktop/Cargo.toml"
printf '[package]\nname = "files-store"\nversion = "0.1.0"\n' > "$W/features/files-store/Cargo.toml"
cat > "$W/Cargo.lock" <<'EOF'
# This file is automatically @generated by Cargo.
version = 4

[[package]]
name = "files-store"
version = "0.1.0"

[[package]]
name = "inventory"
version = "0.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "inventory"
version = "26.9.4"

[[package]]
name = "serde"
version = "26.9.4"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "task-app-desktop"
version = "26.9.4"

[[package]]
name = "task-server"
version = "26.9.4"
dependencies = [
 "files-store",
 "inventory 0.3.0",
 "inventory 26.9.4",
 "serde",
]
EOF
printf '# Changelog\n\n## [26.9.4] - 2026-09-30\n\n- the last committed section\n' > "$W/CHANGELOG.md"
git -C "$W" init -q -b main; git -C "$W" add -A; git -C "$W" commit -qm "initial import"
git -C "$W" tag -a v26.9.3 -m "Task 26.9.3"
echo "// 26.9.4" >> "$W/features/files-store/lib.rs"; git -C "$W" add -A; git -C "$W" commit -qm "chore(release): v26.9.4 (#3)"
git -C "$W" tag -a v26.9.4 -m "Task 26.9.4"
git -C "$W" tag desktop-v0.0.5 -m "the macOS app's own release, before 26.10"
git -C "$W" tag v99.1.0-rc.1 -m "a suffixed tag never counts"
git init -q --bare -b main "$S/origin.git"
git -C "$W" push -q "$S/origin.git" main:main --tags
git clone -q "$S/origin.git" "$S/wc"; cd "$S/wc" || exit 1
R() { bash scripts/release.sh "$@"; }
# land <subject> <file> [<change file name> <front matter lines> <note>]: a squash-merged PR on <line> (default main)
land() {
  local subj="$1" f="$2" line="${LINE:-main}"
  git fetch -q origin; git switch -q --detach "origin/$line"
  mkdir -p "$(dirname "$f")"; echo "// $subj" >> "$f"; git add "$f"
  if [ $# -ge 5 ]; then printf -- '---\n%b---\n\n%b\n' "$4" "$5" > ".changes/$3.md"; git add ".changes/$3.md"; fi
  git commit -qm "$subj"; git push -q origin "HEAD:$line"; }
tagged() { git fetch -q --tags origin; git rev-parse -q --verify "refs/tags/$1^{commit}" 2>/dev/null; }

echo "=== CalVer: the next version"
export RELEASE_MONTH=26.10
eq "tags v26.9.3, v26.9.4 (and desktop-v0.0.5, v99.1.0-rc.1) in October 2026" "$(R next 2>/dev/null)" 26.10.0
eq "the same in September: the next micro" "$(RELEASE_MONTH=26.9 R next 2>/dev/null)" 26.9.5
eq "this month, from the real clock (not before v26.9.4's month)" "$(env -u RELEASE_MONTH bash scripts/release.sh next 2>/dev/null | grep -cE '^[1-9][0-9]\.([1-9]|1[0-2])\.[0-9]+$')" 1
expect_fail "RELEASE_MONTH must be YY.M" env RELEASE_MONTH=26.01 bash scripts/release.sh next

echo "=== the image tag: exactly the bare CalVer (argocd-image-updater's filter and order)"
eq "v26.10.4 -> 26.10.4" "$(R image-tag v26.10.4 2>/dev/null)" 26.10.4
eq "v26.12.10 -> 26.12.10" "$(R image-tag v26.12.10 2>/dev/null)" 26.12.10
for t in 26.10.4 v26.09.1 v26.10.04 v26.13.0 v26.0.1 v6.10.0 v26.10.4-rc.1 desktop-v0.0.5 vfoo; do
  expect_fail "image-tag refuses '$t'" R image-tag "$t"
done

echo "=== change files since the last tag -> the draft notes"
land "feat(files): offline drafts (#70)" features/files-store/a.rs added-drafts '"task-app-desktop" = "minor"\n"files-store" = "patch"\n' "Added offline drafts"
land "fix(sync): reconnect backoff (#71)" features/files-store/b.rs fixed-backoff '"files-store" = "patch"\n' "Fixed reconnect backoff resetting after sleep"
land "feat(server)!: rename sync.peer (#72)" apps/server/src/c.rs renamed-peer '"task-server" = "major"\n' "Renamed config key \`sync.peer\` to \`sync.peers\`\n\nMigration: run \`task-server migrate-config\`; no database migration."
# a sea-orm migration module, as services declare them
git fetch -q origin; git switch -q --detach origin/main
printf 'mod m20261024_000001_create_widgets;\n' >> apps/server/src/migrations.rs; git add -A
printf -- '---\n"task-server" = "minor"\n---\n\nAdded widgets\n' > .changes/added-widgets.md; git add -A
git commit -qm "feat(server): widgets (#77)"; git push -q origin HEAD:main
land "ci: faster plan (#73)" .github/x.yml ci-plan '"files-store" = "none"\n' "CI plans faster"
M_GOOD=$(git rev-parse origin/main)
land "docs: a typo (#74)" docs/typo.md
land "fix(desktop): a late fix (#75)" apps/desktop/late.rs late-fix '"task-app-desktop" = "patch"\n' "Fixed a late bug"
M_HEAD=$(git rev-parse origin/main)
git switch -q --detach origin/main
expect_ok "plan" R plan; R plan > "$S/plan.out" 2>&1
expect_out "plan: next is v26.10.0" "$S/plan.out" 'next release: *v26\.10\.0'
expect_out "plan: six change files since v26.9.4" "$S/plan.out" 'change files since v26\.9\.4: *6'
expect_out "plan: the committed version is not the released one" "$S/plan.out" 'committed version: *26\.9\.4 \(not the released one'
expect_out "plan: the migration since v26.9.4" "$S/plan.out" '^m20261024_000001_create_widgets$'
R notes > "$S/n.md" 2>&1
expect_out "Breaking / Migration holds the major note" "$S/n.md" '^### Breaking / Migration'
expect_out "  ...with its Migration paragraph indented under it" "$S/n.md" '^  Migration: run `task-server migrate-config`'
expect_out "Added holds the minor notes" "$S/n.md" '^- Added offline drafts$'
expect_out "Fixed and improved holds the patch notes" "$S/n.md" '^- Fixed reconnect backoff resetting after sleep$'
expect_out "Database migrations section" "$S/n.md" '^- `m20261024_000001_create_widgets`$'
expect_no  "the none note is skipped" "$S/n.md" 'CI plans faster'
[ "$(grep '^### ' "$S/n.md" | paste -sd'|' -)" = "### Breaking / Migration|### Added|### Fixed and improved|### Database migrations" ] \
  && ok "groups in order: Breaking, Added, Fixed, migrations" || bad "group order: $(grep '^### ' "$S/n.md" | paste -sd'|' -)"
[ "$(grep '^- Fixed' "$S/n.md" | paste -sd'|' -)" = "- Fixed reconnect backoff resetting after sleep|- Fixed a late bug" ] \
  && ok "entries in merge order" || bad "entry order: $(grep '^- Fixed' "$S/n.md" | paste -sd'|' -)"

echo "=== release: what it refuses"
expect_fail "no commit on main has a green CI passed" R release --yes
grep -q "has a green 'CI passed'" "$S/last.out" && ok "  ...saying so" || bad "  ...message"
SHIM_CHECK="completed failure" expect_fail "release <sha>: CI passed red" R release --yes "$M_HEAD"
grep -q "is 'completed failure', not green" "$S/last.out" && ok "  ...naming the check result" || bad "  ...message"
expect_fail "release <sha>: CI passed still running" R release --yes "$M_HEAD"
git switch -q -c feat/sta-9-thing origin/main; echo x > stray.txt; git add stray.txt; git commit -qm "feat: unmerged"; git push -q origin feat/sta-9-thing
STRAY=$(git rev-parse HEAD); green "$STRAY"
expect_fail "release <sha>: a commit on a feature branch (green, but not on main or release/*)" R release --yes "$STRAY"
grep -q 'is on neither origin/main nor an origin/release/v\* line' "$S/last.out" && ok "  ...for that reason" || bad "  ...refused for another reason"
expect_fail "release <sha>: not a commit" R release --yes 0123456789abcdef0123456789abcdef01234567
expect_fail "release: an unknown option" R release --force
eq "nothing was tagged" "$(git ls-remote --tags origin 'v26.10*' | wc -l | tr -d ' ')" 0

echo "=== release: main's newest green commit"
green "$M_GOOD"
SHIM_OTHER="completed cancelled" expect_ok "release --dry-run" R release --dry-run
cp "$S/last.out" "$S/dry.out"
expect_out "  ...picks the newest green commit, not main's head" "$S/dry.out" "^release v26\.10\.0: $(git rev-parse --short "$M_GOOD") ci: faster plan"
expect_out "  ...says why it skipped the newer ones" "$S/dry.out" "skipping $(git rev-parse --short "$M_HEAD") fix\(desktop\): a late fix.*'completed cancelled'"
expect_out "  ...shows the notes" "$S/dry.out" '^- Added offline drafts'
expect_out "  ...and the migrations" "$S/dry.out" 'm20261024_000001_create_widgets'
expect_no  "  ...without the late fix, which is after it" "$S/dry.out" 'Fixed a late bug'
expect_out "  ...and tags nothing" "$S/dry.out" 'dry run: v26\.10\.0 not tagged'
eq "  ...nothing on origin" "$(git ls-remote --tags origin 'v26.10*' | wc -l | tr -d ' ')" 0
expect_ok "release (no terminal: no prompt)" R release
eq "v26.10.0 is on the newest green commit" "$(tagged v26.10.0)" "$M_GOOD"
eq "annotated" "$(git cat-file -t v26.10.0)" tag
eq "main's committed version did not move" "$(R version origin/main)" 26.9.4
eq "nothing was committed for it" "$(git rev-parse origin/main)" "$M_HEAD"
expect_fail "release again: nothing green since v26.10.0" R release --yes
expect_fail "release <the tagged sha>: already released" R release --yes "$M_GOOD"
grep -q 'already released as v26.10.0' "$S/last.out" && ok "  ...naming the tag" || bad "  ...message"
green "$M_GOOD~2"
expect_fail "release <an older sha>: behind v26.10.0" R release --yes "$M_GOOD~2"
grep -q 'is older than v26.10.0 on main' "$S/last.out" && ok "  ...saying so" || bad "  ...message"
green "$M_HEAD"
RELEASE_MONTH=26.9 expect_fail "release: a version not above the highest tag (26.9.5 after v26.10.0)" R release --yes
grep -q 'is not above the highest tag v26.10.0' "$S/last.out" && ok "  ...saying versions only increase" || bad "  ...message"
eq "  ...and tagged nothing" "$(git ls-remote --tags origin 'v26.9.5' | wc -l | tr -d ' ')" 0

echo "=== the next release in the same month: notes since the previous tag only"
expect_ok "release" R release --yes
eq "v26.10.1 on main's head" "$(tagged v26.10.1)" "$M_HEAD"
R notes v26.10.1 > "$S/n1.md" 2>&1
expect_out "v26.10.1's notes: the late fix" "$S/n1.md" '^- Fixed a late bug$'
expect_no  "v26.10.1's notes: nothing v26.10.0 shipped" "$S/n1.md" 'offline drafts|reconnect backoff|sync\.peer|widgets'
R notes v26.10.0 > "$S/n0.md" 2>&1
expect_out "v26.10.0's notes are still its own" "$S/n0.md" '^- Added offline drafts'
expect_no  "  ...without the later fix" "$S/n0.md" 'Fixed a late bug'
eq "next after v26.10.0, v26.10.1 in October" "$(R next 2>/dev/null)" 26.10.2
land "chore: tooling only (#76)" features/files-store/t.rs tooling '"files-store" = "none"\n' "Tooling"
R notes > "$S/nn.md" 2>&1
expect_out "draft with only a none change file: no user-visible changes" "$S/nn.md" '^No user-visible changes'

echo "=== stamp: the release version into a build's checkout (uncommitted)"
git switch -q --detach v26.10.1
: > "$S/github-env"
expect_ok "stamp v26.10.1" env GITHUB_ENV="$S/github-env" bash scripts/release.sh stamp v26.10.1
eq "  ...the version this checkout builds as" "$(R version)" 26.10.1
eq "  ...exported to the job's later steps" "$(cat "$S/github-env")" "TASK_RELEASE_VERSION=26.10.1"
# A changed package version rebuilds every workspace crate (Cargo hashes it
# into every unit), so the stamp must leave the manifests alone.
eq "  ...Cargo.toml still says the committed version" "$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)" 26.9.4
eq "  ...task-server in Cargo.lock did not move" "$(grep -A1 '^name = "task-server"$' Cargo.lock | tail -1)" 'version = "26.9.4"'
eq "  ...only .release-version is new, uncommitted" "$(git status --porcelain --untracked-files=all | awk '{ print $2 }' | paste -sd' ' -)" ".release-version"
eq "  ...HEAD still carries the committed version" "$(R version HEAD)" 26.9.4
expect_ok "stamp again: a no-op" R stamp 26.10.1
expect_fail "stamp refuses a non-X.Y.Z" R stamp 26.10

echo "=== the tag guard (every tag-triggered job, after stamp)"
expect_ok "tag-guard on stamped v26.10.1" env GITHUB_REF=refs/tags/v26.10.1 bash scripts/release.sh tag-guard
expect_ok "tag-guard with GH_TOKEN: CI passed green" env GITHUB_REF=refs/tags/v26.10.1 GH_TOKEN=x bash scripts/release.sh tag-guard
# A tag build's checkout (actions/checkout) rewrites the tag as a plain
# ref to its commit, while origin's is annotated: the guard's fetch must
# take origin's rather than refuse to clobber it (v26.10.2's Deploy did).
eq "  the sandbox's tag is annotated" "$(git cat-file -t refs/tags/v26.10.1)" tag
git update-ref refs/tags/v26.10.1 "$(git rev-parse 'v26.10.1^{commit}')"
expect_ok "tag-guard after the checkout made the tag a plain ref" env GITHUB_REF=refs/tags/v26.10.1 bash scripts/release.sh tag-guard
eq "  ...and origin's annotated tag is back" "$(git cat-file -t refs/tags/v26.10.1)" tag
SHIM_CHECK="completed failure" expect_fail "tag-guard with GH_TOKEN: red CI passed" env GITHUB_REF=refs/tags/v26.10.1 GH_TOKEN=x bash scripts/release.sh tag-guard
expect_fail "tag-guard: a branch ref" env GITHUB_REF=refs/heads/main bash scripts/release.sh tag-guard
expect_fail "tag-guard: the checkout is another tag's" env GITHUB_REF=refs/tags/v26.10.0 bash -c 'bash scripts/release.sh stamp v26.10.0 && bash scripts/release.sh tag-guard'
rm -f .release-version
expect_fail "tag-guard: not stamped (the committed version)" env GITHUB_REF=refs/tags/v26.10.1 bash scripts/release.sh tag-guard
grep -q "run 'release.sh stamp v26.10.1' first" "$S/last.out" && ok "  ...asking for the stamp" || bad "  ...message"
# a tag on a commit no release line has (the feature branch)
git tag v26.10.9 "$STRAY"; git switch -q --detach v26.10.9; R stamp v26.10.9 2>/dev/null
expect_fail "tag-guard: a stray tag on neither main nor release/v*" env GITHUB_REF=refs/tags/v26.10.9 bash scripts/release.sh tag-guard
grep -q 'is on neither main nor a release/v\* line' "$S/last.out" && ok "  ...for that reason" || bad "  ...refused for another reason"
git checkout -q -- .; git switch -q --detach v26.10.1; git tag -d v26.10.9 >/dev/null

echo "=== publish"
expect_ok "publish v26.10.0 (the run's tag)" env GITHUB_REF=refs/tags/v26.10.0 bash scripts/release.sh publish
B="$S/releases/v26.10.0.body"
expect_out "release notes: the change files since v26.9.4" "$B" '^- Added offline drafts'
expect_out "release notes: the migrations" "$B" '^### Database migrations'
expect_out "release notes: what the release carries" "$B" '^### What this release carries'
expect_out "release notes: the images" "$B" 'task-server:26\.10\.0'
expect_out "release notes: on main" "$B" 'on main'
expect_out "release notes: the full diff" "$B" 'compare/v26\.9\.4\.\.\.v26\.10\.0'
eq "release title" "$(cat "$S/releases/v26.10.0.title")" "Task 26.10.0"
expect_ok "publish again: nothing to do" env GITHUB_REF=refs/tags/v26.10.0 bash scripts/release.sh publish
expect_ok "publish v26.10.1 by argument" R publish v26.10.1
expect_out "v26.10.1's Release: its own notes" "$S/releases/v26.10.1.body" '^- Fixed a late bug'
expect_no  "  ...nothing v26.10.0 shipped" "$S/releases/v26.10.1.body" 'offline drafts|widgets'
expect_fail "publish: no tag" R publish v26.10.8

echo "=== hotfix on release/v26.10.1, then main's next release"
land "feat(server): half-done mixer (#83)" apps/server/src/e.rs added-mixer '"task-server" = "minor"\n' "Added the new mixer"
land "fix(desktop): crash on start (#84)" apps/desktop/f.rs fixed-crash '"task-app-desktop" = "patch"\n' "Fixed a crash on start"
FIX=$(git rev-parse origin/main)
land "feat(server): more unshippable work (#85)" apps/server/src/g.rs added-more '"task-server" = "minor"\n' "Added more"
git switch -q --detach origin/main
expect_fail "hotfix: a commit not on main is refused" R hotfix no-issue-stray "$STRAY"
expect_fail "hotfix: a bad name is refused" R hotfix Crash-Fix "$FIX"
expect_ok "hotfix no-issue-crash-on-start $FIX" R hotfix no-issue-crash-on-start "$FIX"
HB="$(git rev-parse --abbrev-ref HEAD)"
eq "hotfix branch name" "$HB" "fix/no-issue-crash-on-start"
eq "release/v26.10.1 created at the tag" "$(git rev-parse origin/release/v26.10.1)" "$(git rev-parse 'v26.10.1^{commit}')"
git log -1 --format=%B > "$S/pick.out"; expect_out "cherry-picked with -x" "$S/pick.out" "cherry picked from commit $FIX"
P=$(lastpr); expect_out "hotfix PR into release/v26.10.1" "$S/prs/$P.meta" '^base=release/v26\.10\.1$'
expect_out "hotfix PR title is the fix's" "$S/prs/$P.meta" '^title=fix\(desktop\): crash on start$'
expect_out "hotfix PR body: then just release <sha>" "$S/prs/$P.body" 'just release <merge sha>'
# the hotfix PR squash-merged into the release line
git switch -q --detach origin/release/v26.10.1; git merge -q --squash "$HB"; git commit -qm "fix(desktop): crash on start (#86)"; git push -q origin HEAD:release/v26.10.1
HOT=$(git rev-parse HEAD)
expect_fail "release <hotfix sha>: its CI passed not green yet" R release --yes "$HOT"
green "$HOT"
expect_ok "release <hotfix sha>" R release --yes "$HOT"
expect_out "  ...on the hotfix line" "$S/last.out" 'on release/v26\.10\.1'
eq "v26.10.2 (the next free number) on the hotfix commit" "$(tagged v26.10.2)" "$HOT"
git merge-base --is-ancestor v26.10.2 origin/main && bad "v26.10.2 is not on main" || ok "v26.10.2 is on the release line, not main"
R notes v26.10.2 > "$S/nh.md" 2>&1
expect_out "hotfix notes: the fix" "$S/nh.md" 'Fixed a crash on start'
expect_no  "hotfix notes: nothing unshipped from main" "$S/nh.md" 'mixer|Added more|Tooling'
git switch -q --detach v26.10.2; R stamp v26.10.2 2>/dev/null
expect_ok "tag-guard on v26.10.2 (release/v* line)" env GITHUB_REF=refs/tags/v26.10.2 GH_TOKEN=x bash scripts/release.sh tag-guard
expect_out "  ...naming the line" "$S/last.out" 'on release/v26\.10\.1'
git checkout -q -- .
eq "no forward-port PR (a release commits nothing)" "$(grep -l '^base=main$' "$S"/prs/*.meta 2>/dev/null | wc -l | tr -d ' ')" 0
# main's next release lists what the hotfix did not ship, and says it covers it
MAIN2=$(git rev-parse origin/main); green "$MAIN2"
expect_ok "release on main after the hotfix" R release --yes
eq "v26.10.3 on main" "$(tagged v26.10.3)" "$MAIN2"
R notes v26.10.3 > "$S/n3.md" 2>&1
expect_out "main's notes: include the hotfix" "$S/n3.md" '^Includes all changes up to v26\.10\.2\.$'
expect_out "main's notes: the mixer" "$S/n3.md" 'Added the new mixer'
expect_no  "main's notes: the hotfixed crash is not listed twice" "$S/n3.md" 'Fixed a crash'
R notes v26.10.2 > "$S/nh2.md" 2>&1
expect_out "the hotfix's notes, read after v26.10.3: still the fix" "$S/nh2.md" 'Fixed a crash on start'
expect_no  "  ...and no 'Includes' line from the later main release" "$S/nh2.md" '^Includes'
eq "next month, after v26.10.x" "$(RELEASE_MONTH=26.11 R next 2>/dev/null)" 26.11.0
expect_fail "release <old hotfix-line sha>: already released" R release --yes "$HOT"

echo "=== there are no release PRs"
expect_rc "release-only: never a release PR" 1 R release-only

echo "=== an unreachable origin fails, never passes"
git remote set-url origin "$S/nowhere.git"
green HEAD
expect_fail "release with origin unreachable" R release --yes
expect_fail "tag-guard with origin unreachable" env GITHUB_REF=refs/tags/v26.10.3 bash -c 'git switch -q --detach v26.10.3 && bash scripts/release.sh stamp v26.10.3 && bash scripts/release.sh tag-guard'
git checkout -q -- .; git remote set-url origin "$S/origin.git"

echo "=== run from a git hook: the hook's repo is left alone"
# Hooks export GIT_DIR and GIT_INDEX_FILE: rerun the sandbox under a canary's
# and check its config, refs and index are unchanged. The nested run skips this.
if [ -z "${RELEASE_SANDBOX_NESTED:-}" ]; then
  C="$S/canary"; git init -q -b main "$C"; echo canary > "$C/f"; git -C "$C" add f; git -C "$C" commit -qm canary
  git -C "$C" remote add origin https://example.invalid/canary.git
  snap() { cat "$C/.git/config"; git -C "$C" for-each-ref; git -C "$C" symbolic-ref HEAD; git -C "$C" ls-files -s; }
  snap > "$S/canary.before"
  expect_ok "the sandbox passes under a hook's GIT_DIR" env RELEASE_SANDBOX_NESTED=1 GIT_DIR="$C/.git" GIT_INDEX_FILE="$C/.git/index" GIT_PREFIX= bash "$HERE/scripts/release-sandbox.sh" "$S/nested"
  snap > "$S/canary.after" 2>&1
  if cmp -s "$S/canary.before" "$S/canary.after"; then ok "the hook's repo: config, refs, HEAD and index unchanged"
  else bad "the hook's repo changed"; diff "$S/canary.before" "$S/canary.after" | sed 's/^/      | /' | head -20; fi
fi

echo; echo "=== RESULT: $pass passed, $failn failed"
[ "$failn" = 0 ]
