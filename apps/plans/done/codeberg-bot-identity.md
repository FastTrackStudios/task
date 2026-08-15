# Codeberg bot identity — per-actor forge attribution

**Status:** implemented (server code); awaiting the codeberg bot account.

The server-driven Task↔forge sync (`apps/server/src/forge_sync.rs`) attributes
each forge action to the identity that owns the task, so human work and agent
work are filterable apart on the forge. This mirrors the Hermes pattern in
`~/.starcommand/.../hermes/wiki-skeleton/forgejo-identities.md` — but codeberg
is hosted, so a bot needs a **real account** (no `forgejo admin user create`).

## How attribution is decided

`forge_sync` picks the forge client per task from the primary assignee's
`workflows_proto::AgentRef` (set by `try_claim` / the `task code` loop):

- assignee `is_agent()` ⇒ **bot** identity (`TASK_FORGEJO_BOT_TOKEN`)
- human-assigned or unassigned ⇒ **human** identity (`TASK_FORGEJO_TOKEN`)

There is no caller-principal in the TaskService RPC today, so the task's
assignee is the actor proxy. Caveat: an agent that creates a task without
self-assigning is attributed to the human until it claims the task.

## Env contract (server)

The bot account is **`fts-agent`** on codeberg.

| Var | Identity | Source |
|---|---|---|
| `TASK_FORGEJO_BASE_URL` | — | `https://codeberg.org` |
| `TASK_FORGEJO_TOKEN` | human (you, `codywright`) | SOPS `cody.codeberg.api_token` |
| `TASK_FORGEJO_BOT_TOKEN` *or* `FTS_CODEBERG_ACCESS_TOKEN` | agent (`fts-agent`) | sops secret `cody/codeberg/fts-codeberg-access-token` |
| `TASK_FORGE_POLL_SECS` | — | inbound poll interval (default 60) |

The server accepts **either** `TASK_FORGEJO_BOT_TOKEN` or
`FTS_CODEBERG_ACCESS_TOKEN`, so the sops-rendered `fts-codeberg.env`
(`config.sops.templates."fts-codeberg.env"`) works as a service
`EnvironmentFile=` with no remapping. When neither is set the bot backend
falls back to the human backend (`forge_agent = forge.clone()`) — agent tasks
still sync, just under the human identity until the token is live.

## Secrets (declared in `~/.flake`, sops-nix)

Defined in the host sops config (the user is wiring these):

- `cody/codeberg/fts-codeberg-access-token` — fts-agent API/git access token
  (owner `cody`, `0400`). The server's agent identity.
- `cody/codeberg/fts-agent` — dedicated SSH **private key** for the fts-agent
  account (public half registered on codeberg). Used as an `IdentityFile` for
  **git push as fts-agent** (the `task code` loop), *not* the issue-sync API.
- `fts-codeberg.env` (sops template) renders:
  - `FTS_CODEBERG_ACCESS_TOKEN=<token>` → consumed by the server (above)
  - `FTS_CODEBERG_GIT_URL=https://fts-agent:<token>@codeberg.org` → for HTTPS
    git ops as fts-agent

## One-time setup (manual — codeberg account)

1. Sign up the `fts-agent` codeberg account.
2. Generate an access token, scopes: `read:organization`, `write:issue`,
   `write:repository`, `read:user`. Register the SSH public key on it.
3. Add `fts-agent` as a **Write** collaborator on each managed repo
   (`FastTrackStudios/Task`, …), or to the `FastTrackStudios` org.
4. Fill the sops secrets above and activate (`nixos-rebuild switch`).
5. Restart the server with the `fts-codeberg.env` env file (or
   `FTS_CODEBERG_ACCESS_TOKEN` exported); per-actor goes live.

## Verify

- Create a task assigned to an agent (`AgentRef::Agent`) in a bound project →
  the codeberg issue is authored by the **bot**.
- Create/own a task as yourself → issue authored by **you**.
- Filter on codeberg by author to separate agent vs human work.

## Follow-ups

- Multi-profile (per-agent accounts like Hermes `forge`/`reviewer`/…) — map
  each `AgentRef::Agent { name }` to its own token instead of one bot.
- True caller-principal attribution would need auth context threaded into the
  TaskService RPC (architect-level change); the assignee proxy avoids that.
- Persisting the bot token into the server's runtime env declaratively (a nix
  module like `task-email-watcher.nix`) rather than a manual launch.
