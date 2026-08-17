# Self-Hosting Task

> **⚠ STALE — do not follow verbatim (flagged 2026-07-27).** Everything
> below the Build section describes an older server: `TASK_VAULT`,
> `TASK_DB_PATH`, `TASK_NEXTCLOUD_CONFIG`, `TASK_SEED_DEMO` and the
> `NEXTCLOUD_*` vars are **not read by `task-server`**, and the smoke-test
> commands (`task sync`, `task people`, `task invoice report`,
> `task server add`) are **not in the CLI**. The NixOS module it documents
> (`nix/module.nix`) is an orphan — nothing in the flake imports it.
>
> For the real thing use:
> - [`../.env.example`](../.env.example) — the complete env-var inventory
>   (`TASK_DATA_ROOT`, `TASK_SERVER_BIND`, `TASK_SERVER_VAULT_ROOT`, the
>   per-service DB URLs, …)
> - [`../deploy/chart/`](../deploy/chart/) — the Helm chart that actually
>   ships, plus `deploy/docker-compose.yml`
> - [`starcommand-webapp-runbook.md`](starcommand-webapp-runbook.md) — the
>   operator runbook for the live deployment
> - [`../ARCHITECTURE.md`](../ARCHITECTURE.md) — what the server is
>
> Rewriting this file against the current server is open work.

Task runs as a normal Rust service over a markdown vault plus per-service
SQLite databases. It exposes architect/vox RPC over a WebSocket at
`/org/{slug}/vox`; clients and agents use those typed services.

## Build

```bash
nix build .#task-server
nix build .#task-cli
```

For development or manual administration:

```bash
nix develop
cargo check -p task-server
cargo check -p task-cli
```

## Starcommand stable/preview runbook

For the Starcommand deployment architecture and operator runbook covering
`task.starcommand.live` and `task-preview.starcommand.live`, see
[`docs/starcommand-webapp-runbook.md`](starcommand-webapp-runbook.md).

## Runtime Files

Recommended layout:

```text
/srv/task/vault/                 # markdown task/project vault
/var/lib/task-server/task.sqlite # auth/org/activity SQLite database
/etc/task/nextcloud.toml         # provider config, secret file references only
```

Set:

```bash
TASK_VAULT=/srv/task/vault
TASK_DB_PATH=/var/lib/task-server/task.sqlite
TASK_NEXTCLOUD_CONFIG=/etc/task/nextcloud.toml
```

`TASK_NEXTCLOUD_CONFIG` should point at a file like
`docs/templates/nextcloud.toml.example`. Prefer `password_file` over putting app
passwords directly in environment variables.

## Smoke Tests

Before enabling unattended sync:

```bash
task doctor --deep
task sync --plan
task sync --state
task people list
task project list
task invoice report
```

Remote client setup:

```bash
task server add home --url https://task.example.com --session-token "$TOKEN" --use-now
task --server home doctor --deep
task --server home sync --plan
```

## NixOS

This flake exports a NixOS module:

```nix
{
  imports = [ inputs.task.nixosModules.task-server ];

  services.task-server = {
    enable = true;
    package = inputs.task.packages.${pkgs.system}.task-server;
    vaultRoot = "/srv/task/vault";
    databasePath = "/var/lib/task-server/task.sqlite";
    port = 3456;
    seedDemo = true;

    nextcloud = {
      enable = true;
      url = "https://cloud.example.com";
      username = "agent";
      passwordFile = config.sops.secrets."task/nextcloud-password".path;
      projectsPath = "Projects/";
      calendar = "tasks";
      eventCalendar = "events";
      deckEnabled = true;
    };

    # Optional: when vaultRoot points at live Nextcloud server storage
    # owned by nextcloud:nextcloud, grant task-server write access without
    # changing ownership.
    nextcloudVaultAcl.enable = true;
  };
}
```

### Nextcloud vault ACLs

If `vaultRoot` points at a live Nextcloud data tree, for example
`/var/lib/nextcloud/data/codywright/files/Projects`, keep ownership with
Nextcloud and grant the service user access with POSIX ACLs:

```nix
services.task-server = {
  vaultRoot = "/var/lib/nextcloud/data/codywright/files/Projects";

  nextcloudVaultAcl = {
    enable = true;
    # path defaults to vaultRoot
    recursive = true;
  };
};
```

When enabled, activation applies `u:<task-server-user>:rwX` to the configured
path and default ACLs to directories so new files inherit service write access.
This preserves Nextcloud-compatible ownership while allowing Task to create and
update project/task files. Keep the ACL root as narrow as practical, normally the
Projects vault root rather than the whole Nextcloud data directory.

## Generic systemd

Use `docs/systemd/task-server.service` as a starting point on non-NixOS hosts.
Keep secrets in files readable only by the service user.
