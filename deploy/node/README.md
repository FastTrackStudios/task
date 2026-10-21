# deploy/node — service files for a headless Task node

The defaults of what `task-server service write <dir>` renders, committed
so they can be read (and diffed) without a binary. A unit test
(`apps/server/src/service_units.rs`) keeps them byte-identical to the
binary's output; regenerate them with

    task-server service write deploy/node/systemd --for systemd
    HOME=/Users/you task-server service write deploy/node/launchd --for launchd

Prefer `task-server service write` on the machine itself: it fills in
that machine's paths (launchd has no `~`).

- `systemd/` — `task-server.service` (the node, `task-server peer`, as
  user `task`, data in `/var/lib/task-server`), `task-server-update.service`
  + `.timer` (daily `task-server self-update`, restart only on a new binary).
- `launchd/` — the per-user agent and its daily update, for macOS.

How to install, pair and update a node: [docs/headless-node.md](../../docs/headless-node.md).
