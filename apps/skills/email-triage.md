---
name: email-triage
description: Sort unprocessed email in the curator's agent inbox into task/project links with optional Proton labels. Use whenever new mail arrives on agent@fasttrackaudio.com (or any agent alias) or when manually prompted to clear backlog.
runs_as: curator
trigger: on-demand or self-scheduled (cron / NC Mail post-sync hook)
---

# Email triage

You are the curator. Your job is to empty the agent inbox by filing
every message either against a task/project (authoritative link) or
as "triaged but no action" (marker only). Nothing stays un-handled.

## Environment

Before any command, ensure these are set:

```
TASK_USER=curator
NEXTCLOUD_URL=https://cloud.starcommand.live
NEXTCLOUD_USER=curator
NEXTCLOUD_PASSWORD=<curator password from /run/secrets/starcommand/selfhost/users/curator/password on starcommand>
```

Curator has two Nextcloud Mail accounts; both are listed by
`task email accounts`:

- account `3` — `agent@fasttrackaudio.com` (primary agent inbox)
- account `4` — `cody@fasttrackaudio.com` (curator's triage view of
  Cody's personal inbox; only touch this when explicitly asked)

## The triage loop

1. `task email sweep --account 3` returns messages in agent@'s INBOX
   that are neither linked to a task/project nor tagged `$processed`.
   Output is JSON; oldest first.

2. For each message, decide one of:

   **(a) link to a task or project.** You chose this when the email
   names a project/client/task or continues a thread already linked:

   ```
   task email link --to task --reference <task-id> \
     --message-id '<Message-Id>' \
     --subject '<subject>' --from '<from>' \
     --date '<date>' \
     --nc-db-id <databaseId> --account-id 3 --mailbox INBOX \
     --imap-uid <uid>
   ```

   Then apply a Proton label and the $processed marker:

   ```
   task email folder-create --account 3 --name 'Labels/project.<slug>'   # once; idempotent
   task email tag set '$project.<slug>' --email-id <databaseId>          # optional, for cross-client visibility
   task email mark-processed --email-id <databaseId> --note 'linked to <task-id>'
   ```

   Note: `<slug>` must not contain `/` (use dots), because NC Mail's
   URL router splits on slashes in imapLabels.

   **(b) mark processed without linking.** Use this for newsletters,
   automated notifications, verification emails, and other messages
   that don't belong to a task:

   ```
   task email mark-processed --email-id <databaseId> --note '<reason>'
   ```

   Optionally file it with `task email move --email-id <databaseId>
   --to-folder <folder-id>` into Archive / Spam / a specific folder.

3. Repeat until `task email sweep --account 3` returns an empty list.

## Decision heuristics (for the `link` branch)

- Sender domain matches a client → the email belongs to that client's
  most recent active project (`task project list --client <slug>`).
- Subject contains a task id (`TASK-NNN` or bracketed slug) → link
  directly to that task.
- In-Reply-To header matches a previously linked Message-Id
  (`task email list --to task <id>`) → continue the same thread's
  linkage.
- Plain meeting invites / calendar updates → link to the task whose
  title matches the meeting agenda, or mark processed if standalone.
- Support / verification emails from services we use → mark
  processed, no link.

When in genuine doubt, **ask Cody via `task comment <task>`** or a
Talk DM; do not guess a link.

## Triggering this skill

Curator runs this skill whenever new mail arrives. Two options:

- **Reactive (recommended)**: `task email watch` subscribes to the
  Bridge IMAP IDLE stream and emits one JSON line per server push.
  Run it as a systemd user service on starcommand (where Bridge is
  loopback). On each event, fire this skill once.

  ```
  # on starcommand, as the curator user's shell
  IMAP_PASSWORD=<bridge_password from SOPS> \
  task email watch \
    --host 127.0.0.1 --port 1143 \
    --user agent@fasttrackaudio.com \
    --mailbox INBOX \
    --ca-bundle /var/lib/nc-mail-trust/ca-bundle.crt \
  | while read -r line; do
      echo "$line" | jq -r '.raw'        # debug
      task email sweep --account 3       # run the triage loop
    done
  ```

  Latency: a few seconds from arrival at Proton to a JSON line.

- **Fallback**: cron every ~5 minutes when IDLE is unavailable
  (e.g. if Bridge restarts or the watcher crashes). Register a
  timer with your scheduler that fires this skill with
  `--account 3`. NC Mail polls Bridge every ~3 min so total latency
  is ~8 min end-to-end.

If you schedule yourself via cron, keep the sweep cheap:
`--limit 20` per run is plenty, as the triage loop is idempotent.

## Error handling

- `401 Unauthorized` — the curator password drifted. Re-read it from
  `/run/secrets/starcommand/selfhost/users/curator/password`.
- `Bridge not reachable` — check `systemctl --user status
  protonmail-bridge` on starcommand; Cody must re-run interactive
  login if the vault was reset.
- `sweep` returns messages with no Message-Id — some system emails
  omit it. Use `--nc-db-id` only for linking; the EmailRef on disk
  still writes an empty Message-Id, which is tolerated.
- Duplicate tag on mark-processed — the call is idempotent; if the
  message is already tagged, NC returns 200 and we no-op.

## Audit trail

Every `link` / `move` / `mark-processed` call writes a row to
`changes.db` on the task vault with `actor = curator`. Cody can
review or rewind via `task activity --since '2 days ago'`.
