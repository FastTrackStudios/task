# The account-driven mount

Sign in once; see every org your account can see.

## What it replaces

Pairing is per org. `task files device pair --org <slug>` hands an org
this machine's endpoint id and prints the org's own, then the sync agent
needs `fts-files-daemon peer <org endpoint>` for each — and every org you
join afterwards is another round of the same. A person in seven orgs did
that seven times, and the mount drifted the moment anything changed.

## What it is

Three pieces, one seam each:

- **Server** — `DeviceEnrollmentService` on `/server/vox`
  (`files-proto`, `service/enrollment.rs`). `enroll_everywhere(session_token,
  endpoint, name)` resolves whose token it is, which orgs that account is a
  member of (issuer-mirrored and local rows alike), and runs the per-org
  enrolment in each — the same `enroll_device` a manual pair runs, so the
  rows are the same rows. It answers each org's endpoint id. It only widens
  what the account already has: an org the token is not in is not touched.
- **Agent** — `fts-files-daemon sign-in` (`files-daemon`, `account.rs`).
  Holds the token (`<data>/account-token`, mode `0600`, beside
  `account.json` naming the server), and on every tick where an enrolment
  is due — the first after sign-in, then every ten minutes — calls
  `enroll_everywhere` with its own endpoint id, admits each org endpoint,
  takes what each offers, and forgets the endpoints of orgs the account no
  longer reaches. `fts-files-daemon enroll` runs a round now; `sign-out`
  forgets the token (orgs already admitted keep syncing until `forget`).
  `status` shows the account, when it last enrolled, and in what.
- **Desktop app** — after its own central sign-in, hands the bearer to the
  agent over the control socket (`sign_in`) and runs a round; the tray has
  *Sync as the signed-in account* and *Stop syncing as this account*.

For a script or a machine without the app: `task files device enroll-all`
is a thin client of the same server call.

## Env

| variable | meaning |
|---|---|
| `FTS_FILES_DAEMON_SERVER` | the Task server the token belongs to; default `wss://task.fasttrackstudio.app` |
| `FTS_FILES_DAEMON_TOKEN` | token for `sign-in` (else `--token`, else stdin) |

## What does not change

The manual verbs (`device pair`, `peer`, `coordinator`, `forget`) keep
working and share the same rows and the same peer set. An account round
never removes a peer it did not add.
