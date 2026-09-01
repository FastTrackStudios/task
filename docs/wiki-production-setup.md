# Wikis on production — setup runbook

How `task.fasttrackstudio.app` gets its wikis, an agent identity that
curates them, and the repo-sourced documentation wikis for the other
FastTrackStudio apps. Written 2026-09-01 against the `wiki-multi-spec`
work; every command below is one the CLI on that branch has.

The order matters: the server must be running an image that has the
wiki lane (a `main` build after PR #29), the agent needs an account
before it can create anything, and a wiki's creator is its first
Editor — so the agent creates the wikis and then grants the human.

## 1. The server

Merging to `main` builds and rolls the image (`.github/workflows/deploy.yml`
→ argocd-image-updater). Confirm the running server is new enough:

```bash
curl -fsS https://task.fasttrackstudio.app/.well-known/task-server.json | head -c 400
```

The cluster environment lives in `~/.starcommand/modules/services/task/task.nix`
and reaches the cluster only through `just gitops-push` there. Two things
this work added and needs:

| env | value | why |
|---|---|---|
| `TASK_TELEMETRY_TEMPO_URL` / `TASK_TELEMETRY_LOKI_URL` | in-cluster Tempo / Loki | the `telemetry_*` MCP tools |
| `TASK_WIKI_DOMAINS` | `fasttrackstudio.app=fasttrackstudios,codywright.fasttrackstudio.app=codywright,…` | the domain a reference carries, per org |

## 2. The agent identity (a person runs this once)

The MCP account lane validates a session against each org's *local* auth
store, so the agent is a local account per org plus membership rows in
the home org (`codywright`). Account creation is a server-binary verb
inside the pod; the existing script does it for every org and prompts
for the password once, with echo off:

```bash
scripts/bootstrap-org-owner.sh --email agent@fasttrackstudio.app --name "Cody Agent" \
  --orgs "codywright,fasttrackstudios,fasttrackaudio,days-to-praise,tombrooksmusic"
```

Then give that one principal membership everywhere it has an account, so
a single session reaches every org through the home-org fallback:

```bash
ssh starcommand 'kubectl -n task exec deploy/task-server -- \
  task-server admin adopt-principal --email agent@fasttrackstudio.app'
```

Sign the agent in to its **own** session file so it never touches the
human's CLI session:

```bash
export TASK_SESSION_FILE=~/.local/share/task-agent/session.json
TASK_PASSWORD='…' task auth login --server https://task.fasttrackstudio.app \
  --org codywright --email agent@fasttrackstudio.app
task auth whoami
```

The token in that file is what goes into the agent's MCP entry
(`~/.claude.json` → `mcpServers.task.headers.Authorization: Bearer <token>`).
Nothing else needs the password; if the session ever expires, reset it
with `task-server admin set-password` in the pod and sign in again.

## 3. The wikis (the agent runs this, as itself)

```bash
export TASK_SESSION_FILE=~/.local/share/task-agent/session.json
S=https://task.fasttrackstudio.app

# FastTrackStudio — public, the ones to share
task wiki create --org fasttrackstudios --server $S --title "Music Theory" --visibility public \
  --purpose "Intervals, scales, modes, harmony and rhythm — the theory the studio teaches and uses."
task wiki create --org fasttrackstudios --server $S --title "Audio Production" --visibility public \
  --purpose "Recording, mixing and mastering technique, and the reasoning behind the studio's defaults."

# Cody Wright — unlisted: reachable by reference, not advertised
task wiki create --org codywright --server $S --title "Bible" --visibility unlisted \
  --purpose "Study notes anchored to the scripture Resource by verse; the Resource holds the text, this holds the reading."
task wiki create --org codywright --server $S --title "Cooking" --visibility unlisted \
  --purpose "Recipes, techniques and the cookbook."

# The creator is the first Editor. Grant the human on each.
CODY=<cody's principal id: `task auth whoami` as Cody, or `admin memberships` in the pod>
for w in music-theory audio-production; do
  task wiki edits grant-editor --org fasttrackstudios --server $S --wiki $w $CODY
done
for w in bible cooking; do
  task wiki edits grant-editor --org codywright --server $S --wiki $w $CODY
done
```

## 4. Documentation wikis mirrored from the repositories

Each product's `docs/` becomes a public wiki in the studio org
(`wiki.source.repo`). The server fetches every ten minutes
(`TASK_WIKI_REPO_SYNC_SECS`) and `refresh-source` fetches now.

```bash
for r in task keyflow signal session; do
  task wiki create --org fasttrackstudios --server $S --title "$r docs" --slug "$r-docs" \
    --visibility public --repo https://github.com/FastTrackStudios/$r.git --branch main --path docs
done
task wiki create --org fasttrackstudios --server $S --title "architect docs" --slug architect-docs \
  --visibility public --repo https://github.com/FastTrackStudios/architect.git --branch main --path docs/content
task wiki list --org fasttrackstudios --server $S
```

An accepted Edit Request on one of these is pushed as a branch and, with
`TASK_GITHUB_TOKEN` in the `task-env` Secret, opened as a pull request;
the wiki shows the change once a sync sees it merged.

## 5. Subscriptions

Cody's vault subscribes to the two studio wikis so `[[Ionian]]`-style
references resolve from personal notes (`wiki.subscribe.resolution`).
The web app's Wiki page has a Subscriptions tab for this; there is no
CLI subcommand yet. Over the wire it is `Subscriptions::subscribe` with
subscriber `vault` and the qualified source
(`fasttrackstudio.app/music-theory`, `fasttrackstudio.app/audio-production`);
`Subscriptions::discover` lists every public wiki on the server.

## What is deliberately not here

- **Assets and song lists** (Days to Praise worship set, FastTrackAudio
  tracks, the Signal sampler assets, the Alan Parsons list for
  TomBrooksMusic) are content, not configuration, and the sources for
  most of them have not been located. The prod tree already holds
  `Assets/days-to-praise/Tracks/` (six songs) and
  `Assets/fasttrackaudio/Tracks/` (one); `Assets/fasttrackaudio/Signal/`
  is empty.
- **Central-issuer identity for the agent.** The org router accepts
  issuer tokens, the MCP account lane does not yet; when it does, the
  agent can move to one issuer account.
