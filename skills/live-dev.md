---
name: live-dev
description: Run the local web app (hot reload) against the deployed Task server and the real central auth issuer, signed in as a real account from .env — so UI and data changes are seen against production data immediately. Also how to drive the same server from the CLI as that account, and the traps that make it look broken.
runs_as: the developer's own account (a test deployment with one user)
trigger: "iterate on the UI against real data", "just live", "set up local dev with prod data", "sign in as me in the dev app"
---

# Live dev — local web app, deployed server, real account

The deployment at `task.fasttrackstudio.app` is a one-user test
environment. The fastest loop for UI work is the local web app pointed
at it: `dx serve` rebuilds on save (~1–2 min for wasm), the page boots
signed in as the account in `.env`, and every RPC hits the real data.

## 1. Credentials in `.env` (gitignored)

```bash
export TASK_LIVE_SERVER="https://task.fasttrackstudio.app"
export TASK_LIVE_EMAIL="acodywright@gmail.com"
export TASK_LIVE_PASSWORD="…"
export TASK_LIVE_NAME="Cody Wright"
```

`.env.example` documents the keys. The password is the **issuer's**
(`https://auth.fasttrackstudio.app`), not the org store's — the two can
differ, and the server on central auth only cares about the issuer.

## 2. Run it

```bash
just live            # :8766 → wss://task.fasttrackstudio.app/vox, signed in as TASK_LIVE_EMAIL
```

What the recipe does (`Justfile`): sources `.env`, bakes
`TASK_VOX_URL_WEB` (the server's vox URL, `https→wss`) and
`TASK_DEMO_CAST=email:password:name:` into the wasm build, and runs
`dx serve --web --addr 127.0.0.1 --port 8766 --hot-patch false`. Debug
builds read the cast (`crates/ui/src/auth.rs`, `option_env!`) and the
login screen lands on that account; the sign-in goes to the issuer over
HTTP (`central_login::password_sign_in`).

Wait for `Build completed` in the log, then open `http://127.0.0.1:8766`.
Edits under `crates/ui` rebuild automatically; watch the log for the
next `Build completed` before reloading.

Run it in the background with the log in the scratchpad:

```bash
env -u RUSTC_WRAPPER just live > $SCRATCH/web.log 2>&1 &
```

## 3. Drive the same server from the CLI as that account

The CLI attaches a bearer only from its session file, and `task auth
login` posts to the org's own store (which may not hold the issuer
password). Mint an issuer session over HTTP and write it in the CLI's
split layout:

```bash
curl -s -X POST https://auth.fasttrackstudio.app/auth/sign-in/email \
  -H 'content-type: application/json' \
  -d '{"email":"'"$TASK_LIVE_EMAIL"'","password":"'"$TASK_LIVE_PASSWORD"'"}' > $SCRATCH/signin.json

python3 - "$SCRATCH" <<'EOF'
import json,sys,os
S=sys.argv[1]; d=json.load(open(f"{S}/signin.json"))
json.dump({"home":"prod","active":"prod",
           "servers":{"prod":{"url":"wss://task.fasttrackstudio.app","slug":"codywright"}}},
          open(f"{S}/session.json","w"))
os.makedirs(f"{S}/session-tokens",exist_ok=True)
json.dump({"token":d["token"],"user_id":d["user"]["id"],"email":d["user"]["email"]},
          open(f"{S}/session-tokens/prod.json","w"))
os.chmod(f"{S}/session-tokens/prod.json",0o600)
EOF

export TASK_SESSION_FILE=$SCRATCH/session.json
task --server https://task.fasttrackstudio.app --org fasttrackstudios wiki list
```

**Always pass `--server`.** Without it the CLI may resolve the default
localhost URL, fail to dial, and silently fall back to the EMBEDDED
backend over `~/.task/orgs/<slug>` — a local clone of the same orgs.
Commands then "succeed" against this machine and the web app never sees
them. Check with `task auth whoami`: the `server:` line must name the
deployment.

The stored `url` must be the `wss://` form (the CLI matches by
authority, but older builds matched the full origin).

## 4. Traps, in the order they bite

| Symptom | Cause | Fix |
|---|---|---|
| Login page, "invalid credentials", console says `central issuer unreachable … peer closed during handshake` | The issuer's vox lane and this client run different vox wire versions; the form fell back to the org store | Already handled: sign-in goes to the issuer over HTTP first. If it recurs, the issuer's CORS must list `http://127.0.0.1:8766` (fts-auth.nix) |
| Login page shows a stale "sign in as alice@acme.test" | Browser `localStorage` from an earlier `just demo` run on the same origin | DevTools → `localStorage.clear(); sessionStorage.clear()`; reload |
| Signed in, but no orgs / nothing loads | The issuer principal has no membership rows (`auth.central = not_a_member`), or `/.well-known` tagged every org `member:false` | `admin adopt-principal --email <addr> --principal <issuer uuid>` in the pod (issuer uuid: `psql -d fts_auth -c "select id,email from auth_users"` on `pg-main-1`); discovery fix shipped 2026-09-02 |
| CLI: `permission denied: anonymous is not a member` | No bearer attached: session file not in split layout, or `url` scheme mismatch | Write `session.json` + `session-tokens/<key>.json` as above; use `wss://` |
| CLI created things the web app cannot see | Embedded fallback (see §3) | `--server`; remove the strays under `~/.task/orgs/<slug>/…` |
| Song playback dead in the live app | `apps/web/Dioxus.toml` proxies `/org`, `/media`, `/blobs` to :18080 | Expected; media is the one thing that does not follow the live server |

## 5. Reading the server while you work

- Spans: TraceQL in Grafana (`grafana.starcommand.live`), or the
  `telemetry_*` MCP tools once the MCP session has reconnected.
- Logs: Loki, `{namespace="task"} != "net_report"`; `kubectl logs` is
  drowned by iroh. `docs/observability.md` has the cookbook.
- `auth.central` on a span says how the token was read: `member`,
  `not_a_member`, `unrecognised`, `issuer_unreachable`.

## Decision boundaries

- The deployment is real data. Create and edit freely as the account;
  do not delete orgs, wikis or vault trees without asking.
- Server-side changes still need a merge to `main` (deploy is
  automatic on push); only the client is live here.
