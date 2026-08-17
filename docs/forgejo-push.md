# Pushing to Forgejo (`git.starcommand.live`)

Practical guide for getting commits + PRs onto our self-hosted Forgejo without fighting the toolchain. Distilled from real failures encountered in the LoroText editor work — every gotcha here has bitten at least once.

## TL;DR

```bash
# One-time setup (per machine)
git config --global credential.helper store
printf 'https://<user>:<token>@git.starcommand.live\n' > ~/.git-credentials
chmod 600 ~/.git-credentials

# Day-to-day
git push -u forgejo-https <branch>          # works for any repo on git.starcommand.live
                                            # add --no-verify if local capn pre-push fails on env (see below)

# Open + merge a PR when branch protection blocks UI merges
PR=$(curl -s -u <user>:<token> -X POST -H 'Content-Type: application/json' \
  -d '{"title":"…","body":"…","head":"<branch>","base":"main"}' \
  https://git.starcommand.live/api/v1/repos/FastTrackStudios/task/pulls \
  | python3 -c 'import sys,json;print(json.load(sys.stdin)["number"])')
curl -s -u <user>:<token> -X POST -H 'Content-Type: application/json' \
  -d '{"Do":"merge","force_merge":true}' \
  "https://git.starcommand.live/api/v1/repos/FastTrackStudios/task/pulls/$PR/merge"
```

## Remotes

Every clone of this repo ships with three remotes. Pick by what you need:

| Name | URL | When to use |
| --- | --- | --- |
| `forgejo-https` | `https://git.starcommand.live/FastTrackStudios/task.git` | Default for push/pull. HTTPS works everywhere; needs creds in `~/.git-credentials` or an asked-for prompt. |
| `origin` | `git@git.starcommand.live:FastTrackStudios/task.git` | Only if SSH port 22 is reachable from your machine. Often blocked on locked-down networks or sandboxed agent environments. |
| `github` | `git@github.com:FastTrackStudios/task.git` | Mirror; do not push primary work here. |

If you see `ssh: connect to host git.starcommand.live port 22: Network is unreachable`, your environment blocks port 22 — use `forgejo-https` instead of `origin`.

## Credentials

`git.starcommand.live` runs Forgejo, which accepts HTTPS basic auth with either:

- **Username + password.** Works today but breaks the moment you turn on 2FA.
- **Username + Personal Access Token.** Recommended. Generate at Forgejo → Settings → Applications → "Generate New Token" with scope `write:repository` (add `write:issue` if you'll be opening PRs / commenting via API). Forgejo PATs look like `c34664066544a9f19ff6a04be3e24013e5e27588`.

Store them with the `store` credential helper:

```bash
git config --global credential.helper store
printf 'https://<user>:<token>@git.starcommand.live\n' > ~/.git-credentials
chmod 600 ~/.git-credentials
```

One line covers every repo on `git.starcommand.live` (Task, architect, codywright/anything). Both `cody` and `codywright` are valid Forgejo logins; check `~/.gitconfig` for which name you've used elsewhere — Forgejo rejects mismatched casing.

Bot identities (e.g. `forge`, `hermes`) keep their tokens at `/home/agent/.hermes/forgejo-tokens/<name>.env`. Sourcing one of those `.env` files gives you `FORGEJO_TOKEN_OWNER` + the token, which you can hand to git as either an in-URL embed:

```bash
git push "https://$FORGEJO_TOKEN_OWNER:$FORGEJO_TOKEN@git.starcommand.live/<owner>/<repo>.git" <branch>
```

or store under a different cred entry. Don't mix bot creds into your personal `~/.git-credentials` — service identities should stay scoped to the agent that owns them.

## Push hooks

Two layers can block a push. Both have distinct fixes.

### Server-side (Forgejo branch protection)

`main`, `staging`, and `dev` on `FastTrackStudios/task` require:

- 1 approval from `codywright` or `reviewer`
- Merge whitelist: `codywright` or `devops` team

`apply_to_admins` is **false**, so admins can bypass with `force_merge`. The UI doesn't expose this — use the API:

```bash
curl -s -u <user>:<token> -X POST -H 'Content-Type: application/json' \
  -d '{"Do":"merge","force_merge":true}' \
  "https://git.starcommand.live/api/v1/repos/FastTrackStudios/task/pulls/<num>/merge"
```

A 200 means merged. Forgejo will *not* let you self-approve your own PR (HTTP 422), so for solo work `force_merge` is the path. For team work, ping a reviewer and use the normal merge button.

### Client-side (capn pre-push)

`apps/task/hooks/pre-push` (installed via `just install-hooks`, which sets `core.hooksPath`) runs [`capn pre-push`](https://github.com/anthropics/capn), which builds the workspace before letting the push through. Capn needs the system libs every workspace member pulls in — `pango`, `webkitgtk_4_1`, `gtk3`, `libsoup_3`, the gstreamer stack — because `apps/task/desktop` depends on `dioxus-desktop`.

The hook handles this for you: outside a nix shell it re-enters `nix develop` before invoking capn, so the GTK + WebView dep list is on `PKG_CONFIG_PATH` automatically. Inside a nix shell (`IN_NIX_SHELL` set) it runs capn directly in the current env.

If capn still fails:

1. **The failure is about your diff.** Fix it. Capn caught a real regression — clippy lint, format drift, broken test. This is the path 95% of the time.
2. **A new workspace member needs a system lib that isn't in the dev shell.** Add it to dioxus-flake's `buildInputs` (or this repo's flake if we ever decouple). Don't paper over with `nix-shell -p <lib>` — the next agent will hit the same wall.
3. **The error genuinely has nothing to do with your changes and isn't a dep gap** (e.g. transient compiler ICE, network hiccup pulling a registry). `NO_CAPN=1 git push …` bypasses capn for the one push. Reserve for actual infrastructure problems; never for "capn keeps complaining about my code."

The `--no-verify` / `NO_CAPN` escape is the last resort, not the first. The whole point of pre-push is to catch what CI would catch, before pushing — skipping it just moves the failure to PR review.

## Working from a non-`main` worktree

The repo lives in a git worktree setup. `/home/cody/Development/Task-architect` is one worktree on `feat/architect-migration`; `main` lives at a different worktree (or no worktree at all, just the bare ref under `/home/cody/Development/Task/.git`).

Fast-forwarding local `main` after a remote merge — without checking it out — uses the `refspec` form:

```bash
git fetch forgejo-https main:main
```

This updates the local `main` ref directly. You can run it from any worktree.

To list worktrees: `git worktree list`. Each `.claude/worktrees/<topic>` is a transient agent worktree; pushes from there land on the same remote with the same creds.

## Architect repo (`codywright/architect`)

Separate Forgejo project, separate URL: `https://git.starcommand.live/codywright/architect.git`. The single `~/.git-credentials` line covers it — `git.starcommand.live` is the host, the path is per-repo. No branch protection on `main` there yet, so a normal `git push origin main` works once auth is set.

## When the push silently looks like it's hanging

In order of likelihood:

1. **`capn pre-push` is compiling.** Check `ps aux | grep capn`. The first push of a session can take a minute on a cold cache.
2. **First push of a feature branch with lots of history.** `git log --oneline main..HEAD | wc -l` tells you how many commits are being uploaded.
3. **Forgejo is overloaded.** Less common; `curl https://git.starcommand.live/api/v1/version` returns instantly when healthy.
4. **The wrong remote.** `origin` over SSH from an env without port 22 access hangs until TCP gives up. `git remote -v` and pick `forgejo-https` instead.

## See also

- [`docs/self-host.md`](self-host.md) — running Task as a server, separate from how its source code is hosted.
- [`docs/starcommand-webapp-runbook.md`](starcommand-webapp-runbook.md) — full Starcommand deployment shape including Forgejo + the rest of the platform.
