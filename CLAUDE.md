# Working in this repo

## The dev loop: server + webapp, owned by you

You (the agent) own the lifecycle of the demo processes: launch them
yourself as background tasks, read their logs, and restart them after
rebuilding. Don't ask the user to run them.

```bash
just demo serve    # both example servers; plants the seed idempotently first
just demo web      # webapp via dx serve on :8766 (proxies /org,/media,/blobs → :18080)
just demo desktop  # native desktop app (frameless chrome; window pos via TASK_WINDOW_POS)
just demo telemetry  # local grafana/otel-lgtm on :3000, OTLP :4318 — servers auto-attach
```

Ports (fixed — the dx proxy in `apps/web/Dioxus.toml` is committed against them):

| what | port |
|---|---|
| ACME Audio server (`DEFAULT_VOX_URL`, proxy target) | 18080 |
| VNT Video server (federation peer) | 9102 |
| webapp (dx serve) | 8766 |
| Grafana / OTLP | 3000 / 4318 |

Notes that save time:

- **Plant is idempotent** — `serve` re-plants on every start; files already
  on disk are kept, missing ones (and reconciliations like part order) are
  topped up. To reseed from scratch, delete the demo data roots the script
  prints and serve again.
- **ffmpeg must be on PATH when planting** or video deliverables stay
  outstanding (the dev shell carries it; `nix/modules/toolchain.nix`).
- **Stale shells**: if a `dx` launch fails with a compiler-wrapper error,
  run it as `env -u RUSTC_WRAPPER dx ...` (sccache was removed; old shells
  may still export the wrapper).
- Run each process with `run_in_background` and tee output to the
  scratchpad (e.g. `serve.log`, `web.log`) so you can read it later.
- Never `pkill -f "dx serve"` — the pattern matches the user's own panes.
  Kill your recorded PIDs.

## Screenshotting and testing the UI

Use the Chrome browser tools against `http://localhost:8766`. Debug builds
auto-sign-in as Alice via `TASK_DEMO_CAST` (baked by the demo script);
logout shows the cast picker with every seeded account (password in
`example_org::PASSWORD`). The loop:

1. `just demo serve` + `just demo web` (background, logs in scratchpad).
2. Wait for dx to report the build served, then open/navigate a tab.
3. Screenshot, interact, read console/network on failures.
4. After a code change dx rebuilds on save — wait for the rebuild line in
   `web.log` before re-screenshotting.

For the desktop app, `just demo desktop`; `.env` pins the window to the
right display (`TASK_WINDOW_POS`, `TASK_WINDOW_FULLSCREEN`).

## Policy: everything lives in the suite and the seed

**All development must be represented in the integration test suite AND in
the seeded example vault.** A feature that exists only in the UI, or only
in a unit test, is not done:

- **Integration suite** — `tests/integration` (plus feature e2e tests like
  `apps/server/tests`). New behavior gets a scenario stage or e2e test
  that exercises it through the router/transport, not the backend.
- **Seeded example** — `examples/studio` (committed tree) +
  `apps/server/src/example_org.rs` (`DECLARED`, the cast, deliverables,
  tasks). If a demo user can't reach the feature from the planted world,
  extend the seed so they can.

The seed is a contract, enforced by `example_org::declared_tests`
(`cargo nextest run -p task-server -E 'test(declared_tests)'`): every
declared project/part must have its committed tree, every audio
deliverable a committed song folder (`Resources/songs/<slug>/manifest.json`
— slugs via `example_org::song_slug`), every capability in the
vocabulary. Video deliverables are intentionally *not* committed — the
seeder generates them with ffmpeg at plant time, into the project's own
directory (`files/Projects/<dir>/Deliverables/`), and adopts each
declared project directory as a File Root named after the project
(`apps/server/tests/demo_plant.rs` pins all of this, including the
two-version history each fresh video gets: rough cut checkpointed,
final rendered over it, checkpointed again). That root is what makes
deliverables reviewable: video plays through the review platform
(`files_ui::review::{MiniPlayer, ReviewScreen}` — renditions, frame
comments, version compare), audio through the global player
(`task-player-ui`) with a Review door into the same surface (waveform
stage from the Peaks rendition). Never embed a bare
`<video>`/`<audio>` element for deliverable media.

Committed audio is generated reproducibly by
`python3 examples/studio/tools/gen_audio.py` (12 kHz mono WAVs, small
enough for git). Add new songs there, never by hand.

## The PR gate

`checks` runs on THEBATTLESHIP (self-hosted, four slots), against a target
dir that persists between runs. The check set lives in the `Justfile`, one
recipe per check, and the workflow invokes those same recipes — so the two
cannot drift:

```bash
just ci        # ci-fmt, ci-manifests, ci-tests, ci-clippy, ci-wasm
```

Run it before pushing. It is the gate's set in the gate's order, so a pass
here is the gate's answer for the price of a local build.

Do not wait on the gate — push and let it merge itself:

```bash
gh pr create --fill && gh pr merge --auto --squash
```

**`[skip checks]`** in a commit message turns the gate off for that PR, for
when `just ci` already passed against that exact tree. Two limits, both real:

- **Pull requests only.** A push to main runs the full gate whatever the
  message says, because main is what deploys.
- **It is a claim, not a proof.** A PR run builds the MERGE of the branch
  with main; `just ci` built the branch alone. If main moved under you, a
  local pass does not cover the merge. Use it for a follow-up commit on a
  branch you just verified, not for the first push of a stale branch.

Never use GitHub's own skip tokens here — the bracketed `skip ci`,
`ci skip`, `no ci`, `skip actions` and `actions skip` forms. They are
intercepted before any workflow starts and would suppress `deploy.yml`
too.

And never write one of those tokens in a commit message *at all*, even to
say not to use it: GitHub scans the whole message, subject and body, so a
line explaining the trap springs it. (This is not hypothetical — the
commit that added this section did exactly that and silently produced no
CI run.) Refer to them unbracketed, as above.

## Specs and coverage

Rules live in `docs/spec/`; coverage is tracked with `t[impl]`/`t[verify]`
markers and the gaps in `docs/spec/unmet.md`. When you meet a rule, update
both.

## Observability

The span is the wide event: enrich it with `architect_telemetry::wide::set`,
never scatter log or print lines. Field registry, hard rules, TraceQL/LogQL
cookbook and the `telemetry_*` MCP tools: `docs/observability.md`.

## File writing

Use the Write/Edit tools for file creation — never python or shell
heredocs.
