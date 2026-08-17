# The demo

Two companies, two servers, two iroh endpoints, one project across the
boundary. This is `examples/studio` — the tree the integration suite
reads and the four people it hires — planted on disk and served, so the
world the tests assert against is a world you can sign into.

```sh
just demo          # plant both orgs (idempotent; `fresh` wipes first)
just demo serve    # both servers — leave this running
just demo web      # in a second terminal: the web app against ACME
just demo ids      # each org's endpoint id
```

Sign in as `alice@acme.test` / `correct-horse-battery-staple`.

Everything lives under `~/.local/share/task-demo` and is throwaway.
`TASK_DEMO_ROOT` moves it.

## Which local world you want

There are two, and they answer different questions.

| | `just dev-seed` | `just demo` |
|---|---|---|
| shape | several orgs, one server, one process | one org per server, two servers |
| data | synthesised — 50 projects, albums, ffmpeg media | `examples/studio`, the committed studio disk |
| people | one owner with every role | four, each holding something different |
| federation | none | the point |
| use it for | eyeballing multi-org UI, lots of content | anything about two companies, grants, or the wire |

`dev-seed` is the bigger, richer single-server world. Use `demo` when
what you are working on involves an org boundary — sharing, admission,
peering, client access — because inside one process those all work for
reasons that will not survive being deployed. Both federation bugs this
repo has actually had were "it worked because both sides shared a
process".

## The cast

| who | org | holds | where |
|---|---|---|---|
| Alice | ACME Audio | owner | everything, including the guest list |
| Sam | ACME Audio | employee | the work, but cannot widen the guest list |
| Casey | — | client | `Deliverables`, read and comment, no download |
| Victor | VNT Video | owner | everything of VNT's |

All four use the same password. They are defined once, in
`apps/server/src/example_org.rs`, and the integration suite hires the
same four from the same list — so what `tests/integration/tests/people.rs`
proves about Casey is true of the Casey you can log in as.

Casey is the one to look at. `Comment` without `Download` is a client who
can review the mix and not keep it, and the `Deliverables` scope is the
difference between a client link and an org membership.

**One known gap, asserted on purpose.** Per-path access exists only in
Files. Every other lane — projects, tasks, notes, wiki — is gated by the
coarse permit table, which asks only "is this a validated user". So Casey
can list ACME's projects. `office.rs` asserts that deliberately, so
closing it is a decision someone makes and sees fail there rather than a
surprise.

## What is not done for you

The seeder plants trees and accounts. It does not adopt anything, offer
anything or accept anything — those are calls a signed-in person makes
over the wire, and baking them in would hide the half of the demo worth
watching. Each org's projects are sitting on disk waiting:

```
acme-audio/files/Projects/Example Album
acme-audio/files/Projects/First Single - Example Client
vnt-video/files/Projects/Example Documentary - First Client, Second Client
vnt-video/files/Projects/Shared Project
```

Adopting one is the first thing to do in the app.

## Endpoint ids

Each org binds its own iroh endpoint and its id is the whole address —
no host, no port, no certificate. `just demo ids` prints them; they are
also at `orgs/<slug>/iroh-endpoint-id` and in the server log at boot.

The id is stable across restarts because the key beside it is on disk
(`orgs/<slug>/iroh-key.ed25519`, 0600). That is what makes registering a
device against an id meaningful — an id with a per-boot lifetime would
make every registration a lie with a short expiry.

Losing that key loses the org's address, not its data. Every device
registered against it has to be re-registered.

### How they find each other with no internet

A deployed endpoint publishes itself to n0's DNS as it binds and is
dialled by bare id from anywhere. That needs the internet, which a demo
on a laptop may not have — so `just demo serve` sets
`TASK_IROH_PEER_DIR`, a directory both servers write their own address
into and read each other's out of.

It is a stand-in for discovery, sitting on the `MemoryLookup` seam iroh
already provides. Everything above it still dials by bare id and cannot
tell the difference — which is the property that matters, because it
means the demo exercises the same dialler a deployment runs
(`files::IrohRemotes`) rather than a test-only path.

Do not set it in a deployment. If you want to, what you actually want is
n0 discovery working, or a LAN address-lookup service.

## Ports

| | HTTP | iroh |
|---|---|---|
| ACME Audio | 9101 | its endpoint id |
| VNT Video | 9102 | its endpoint id |
| web app | 8766 | — |

`ACME_PORT`, `VNT_PORT` and `WEB_PORT` override the HTTP ones. The iroh
endpoints have no port to configure, which is the point of them.

HTTP is still there because the browser needs it: the web app speaks vox
over a WebSocket, and iroh's client half compiles to wasm but nothing in
this repo dials it from the browser yet. Server-to-server and
device-to-device are iroh only.
