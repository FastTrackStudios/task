# Starcommand Task Webapp Deployment Runbook

This runbook covers the Starcommand deployment shape for Task's stable and live-preview webapp surfaces.

Status: documentation for the Starcommand scaffold tracked by Forgejo issues `FastTrackStudios/task#1` through `#3` and `codywright/starcommand#1` through `#3`. The implementation gates are separate DevOps/reviewer tasks; do not deploy from this runbook until those diffs have been reviewed.

## Architecture overview

| Surface | Public URL | Purpose | Stability policy |
| --- | --- | --- | --- |
| Stable | `https://task.starcommand.live` | Production Task webapp/API/Vox endpoint for normal use. | Uptime wins; use reviewed packages/config only. |
| Preview | `https://task-preview.starcommand.live` | Rapid-iteration/dev-mode preview for Task webapp changes. | May restart frequently, but must not write stable state/config. |

Both surfaces reverse-proxy to a local `task-server` process on Starcommand. `task-server` serves:

- WebSocket Vox RPC at `/vox`.
- Better Auth routes under `/api/auth/*`.
- Metadata and health endpoints under `/api/info`, `/api/servers`, `/api/organizations/routes`, and `/api/health`.

The webapp should use the same public origin for HTTP auth/API calls and WebSocket Vox calls:

```text
Stable HTTP base:   https://task.starcommand.live
Stable Vox URL:     wss://task.starcommand.live/vox?token=$TASK_SESSION_TOKEN&organization_id=$TASK_ORGANIZATION_ID
Preview HTTP base:  https://task-preview.starcommand.live
Preview Vox URL:    wss://task-preview.starcommand.live/vox?token=$TASK_SESSION_TOKEN&organization_id=$TASK_ORGANIZATION_ID
```

For the CLI, configure the matching server URL and credentials:

```bash
export TASK_SERVER=https://task.starcommand.live
export TASK_SESSION_TOKEN='<better-auth-session-token>'
export TASK_ORGANIZATION_ID='org_fts'
task --server "$TASK_SERVER" doctor --deep
```

Preview uses the same client shape with `TASK_SERVER=https://task-preview.starcommand.live`.

## Ports, units, state, logs, and health

Final Starcommand implementation diffs are the source of truth for exact unit names and ports. The architecture target is:

| Item | Stable | Preview |
| --- | --- | --- |
| Public host | `task.starcommand.live` | `task-preview.starcommand.live` |
| Systemd unit | `task-server.service` or `task-web.service` | `task-preview.service` or equivalent |
| Local bind | `127.0.0.1:3456` | a different loopback port, for example `127.0.0.1:3457` |
| `PUBLIC_BASE_URL` | `https://task.starcommand.live` | `https://task-preview.starcommand.live` |
| `SERVER_NAME` | `starcommand` | `starcommand-preview` |
| `SERVER_ID` | `starcommand-task` | `starcommand-task-preview` |
| Vault/state | live Projects vault only when stable is intentionally enabled | isolated preview vault/state only |
| SQLite DB | `/var/lib/task-server/task.sqlite` | separate preview DB, for example `/var/lib/task-preview/task.sqlite` |
| Logs | `journalctl -u <stable-unit>` | `journalctl -u <preview-unit>` |
| Health | `curl -fsS https://task.starcommand.live/api/health` | `curl -fsS https://task-preview.starcommand.live/api/health` |

Never point preview at stable's database, auth secret file, Nextcloud app password file, or writable stable vault path.

Expected runtime environment variables used by `task-server` include:

```bash
BIND_ADDR=127.0.0.1:3456
PUBLIC_BASE_URL=https://task.starcommand.live
SERVER_NAME=starcommand
SERVER_ID=starcommand-task
AUTH_SECRET_FILE=/run/secrets/...          # Nix/systemd may project this into AUTH_SECRET
TASK_DB_PATH=/var/lib/task-server/task.sqlite
TASK_SEED_DEMO=0
TASK_NEXTCLOUD_BASE_URL=https://cloud.starcommand.live
TASK_NEXTCLOUD_USERNAME=codywright
TASK_NEXTCLOUD_APP_PASSWORD_FILE=/run/secrets/...
TASK_TALK_URL=https://cloud.starcommand.live
TASK_TALK_USERNAME=...
TASK_TALK_PASSWORD_FILE=/run/secrets/...
TASK_MAIL_URL=...
TASK_MAIL_USERNAME=...
TASK_MAIL_PASSWORD_FILE=/run/secrets/...
```

Use the names actually emitted by the Starcommand Nix module for secret-file projection; do not paste secret values into shell history or documentation.

## Reverse proxy requirements

The reverse proxy must route each public host to its matching local port and preserve WebSocket upgrades for `/vox`.

Nginx-style requirements:

```nginx
proxy_set_header Host $host;
proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
proxy_set_header X-Forwarded-Proto $scheme;
proxy_set_header Upgrade $http_upgrade;
proxy_set_header Connection $connection_upgrade;
proxy_http_version 1.1;
```

If Vox connections fail while HTTP health succeeds, check the proxy's WebSocket upgrade handling before debugging application code.

## Build and package commands

From the Task repository on THEBATTLESHIP:

```bash
cd /home/cody/Development/Task
nix --extra-experimental-features 'nix-command flakes' build --no-write-lock-file .#task-cli .#task-server
nix --extra-experimental-features 'nix-command flakes' develop -c cargo test -p task-server
```

For a broader pre-release check:

```bash
cd /home/cody/Development/Task
nix --extra-experimental-features 'nix-command flakes' develop -c cargo test --workspace
```

The stable Starcommand service should consume the reviewed `task-server` flake package, not a mutable dev checkout.

## Preview dev-mode commands

Preview is allowed to use a development worktree or dev shell only when it remains isolated from stable state. A safe local smoke pattern is:

```bash
cd /home/cody/Development/Task
export BIND_ADDR=127.0.0.1:3457
export PUBLIC_BASE_URL=https://task-preview.starcommand.live
export SERVER_NAME=starcommand-preview
export SERVER_ID=starcommand-task-preview
export TASK_DB_PATH=/var/lib/task-preview/task.sqlite
export TASK_SEED_DEMO=0
nix --extra-experimental-features 'nix-command flakes' develop -c cargo run -p task-server
```

Operator-managed preview should be run through the Starcommand Nix/systemd unit instead of a long-lived manual shell:

```bash
sudo systemctl restart task-preview.service
sudo journalctl -u task-preview.service -f
curl -fsS https://task-preview.starcommand.live/api/health | jq .
```

## Deployment process

Deploy Starcommand services only from THEBATTLESHIP and only from the owning repo:

```bash
ssh thebattleship
cd /home/cody/.starcommand
git status --short --branch
just build
just deploy
```

Rules:

1. Documentation-only commits may be pushed/reviewed without deploying.
2. Do not run `just deploy` until the Starcommand stable/preview implementation and this runbook have passed reviewer gating.
3. Use the repo-native `just deploy` flow; do not call raw `deploy-rs` unless the Starcommand maintainers intentionally change the repo interface.
4. Keep deploy-rs rollback enabled. Do not use no-rollback unless a separate human-approved recovery-risk exception exists.
5. Stable uptime wins. If preview and stable compete for a port, secret, writable state path, or reverse-proxy host, stop and fix preview.

Rollback policy:

- Prefer a normal redeploy of the previous known-good Starcommand commit using `just deploy`.
- If health checks fail after deployment, gather logs first, then revert or disable the new service in the Starcommand repo and redeploy.
- Do not delete `/var/lib/task-server`, live Nextcloud Projects data, or secret material as part of rollback.

## Operator checks

After deployment or restart:

```bash
curl -fsS https://task.starcommand.live/api/health | jq .
curl -fsS https://task.starcommand.live/api/info | jq .
curl -fsS https://task-preview.starcommand.live/api/health | jq .
curl -fsS https://task-preview.starcommand.live/api/info | jq .
```

Systemd inspection:

```bash
systemctl status task-server.service
journalctl -u task-server.service -n 200 --no-pager
systemctl status task-preview.service
journalctl -u task-preview.service -n 200 --no-pager
```

If the final unit names differ, use the names from the Starcommand Nix diff and update this runbook in the same PR.

## Troubleshooting

### HTTP health fails

1. Check the service is running:

   ```bash
   systemctl status task-server.service
   journalctl -u task-server.service -n 200 --no-pager
   ```

2. Check the local listener/port from the Starcommand host:

   ```bash
   curl -fsS http://127.0.0.1:3456/api/health | jq .
   ```

3. If local health passes but public health fails, inspect the reverse proxy/TLS host mapping.

### Vox/WebSocket fails but health passes

1. Confirm the client uses `wss://<host>/vox`, not `https://<host>/vox`.
2. Confirm the session token and `organization_id` query parameter are present.
3. Inspect proxy upgrade headers and HTTP/1.1 proxying.
4. Tail service logs while reconnecting:

   ```bash
   journalctl -u task-server.service -f
   ```

### Auth/API calls fail

- Better Auth routes live under `/api/auth/*`.
- Confirm `PUBLIC_BASE_URL` exactly matches the public origin for the surface.
- Confirm stable and preview use separate auth secret/state unless preview is deliberately pointed at throwaway test data.

### Config/secrets lookup

- Starcommand Nix configuration lives in `/home/cody/.starcommand`.
- Starcommand secrets are projected by the repo's SOPS/shb secret wiring. Inspect Nix references to secret names, not secret contents.
- Runtime state directories are under `/var/lib/<unit-or-service-name>`.
- Live Nextcloud user files are under `/var/lib/nextcloud/data/codywright/files`; preview must not write there unless explicitly configured for a disposable preview path.

## Safety checklist

Before approving deployment, verify:

- Stable and preview have different loopback ports.
- Stable and preview have different systemd units.
- Stable and preview have different SQLite database paths.
- Stable and preview have different auth secret/config projections or preview uses disposable credentials.
- Preview does not write the stable Projects vault or stable Nextcloud config by default.
- Reverse proxy has distinct vhosts for `task.starcommand.live` and `task-preview.starcommand.live`.
- `/vox` WebSocket upgrades are configured for both hosts.
- `just build` passes in `/home/cody/.starcommand`.
- No `just deploy` happens before reviewer approval.
