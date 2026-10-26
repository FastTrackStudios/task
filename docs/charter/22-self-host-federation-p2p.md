# Self-Hostable - Federated Servers - Peer to Peer

_Charter #22 (formerly issue #22)._

Where the system runs, who can run it, and how instances reach each other — self-hosting as a first-class outcome rather than a grudging option.

Three layers, deliberately in one charter because they are the same question at different scales: your own server, other people's servers, and no server at all.

## Hard requirements

_To be detailed._

Prompts worth answering:

**Self-hostable.** What is the smallest deployment that works — a binary, a container, a Helm chart? What must a self-hoster never need (our cloud, our registry, our keys)? How do upgrades and migrations work without an operator? What is the backup and restore story, and has it been tested by restoring? What breaks if the server is offline for a week?

**Federated.** What crosses a server boundary — a share, a project, an identity, a whole org? How does a person on one server address a person on another? What does trust between servers mean, and who grants it? What happens when a remote server disappears mid-collaboration? Does federation ever require a central directory?

**Peer to peer.** What works with no server in the path at all? Given the chunk store already rides iroh/QUIC, when should two devices transfer directly rather than through the cluster? How are peers discovered and authorised? Is P2P an optimisation or a supported mode of operation?

## Non-goals

_To be detailed._

## Current state

Self-hosting is real: per-org data roots, a Helm chart, and a documented self-hoster example. Content-addressed blobs ride `iroh_blobs` (QUIC), so direct peer transfer is within reach; a sync daemon with device identity and selective sync already exists. Cross-org identity works within one server via a memberships store. Federation is designed but explicitly deferred — the federated-account plan is Phase 3, with a server-brokered locker.

Related: [A Single Trusted System](02-single-trusted-system.md), [The Ideal File Manager](03-file-manager.md).
