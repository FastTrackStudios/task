# Local First - Multiplayer Real-Time Collaborative

_Charter #25 (formerly issue #25)._

The hardest pair of promises in the system, and they pull against each other: **your data is local and works with no network**, and **two people can edit the same thing at the same time and both see it live**.

Most systems pick one. This charter says what it means to have both, and what we give up where they genuinely conflict.

## Hard requirements

_To be detailed._

Prompts worth answering:

**Local first.** What works with the network entirely gone — read, write, search, everything? Is the local copy authoritative, or a cache that thinks it is? How long can a device stay offline before reconciliation becomes untrustworthy? What does the app do the moment it comes back — and does the user have to think about it?

**Multiplayer.** Which surfaces are genuinely concurrent — a note, a task list, a setlist, a file tree, a review? What does presence show, and does it ever leak more than intended? What happens when two people edit the same paragraph, the same task's status, the same file's bytes? Which conflicts can merge silently, which must be shown, and which must never be auto-resolved?

**Where they fight.** A CRDT can merge text; it cannot merge intent. What is the rule when offline edits and live edits disagree — last writer, both preserved, or ask? Does a long-offline device get to overwrite a week of other people's work? Is there a class of data (money, permissions, sends) where offline writes are simply refused?

## The test

Two people, one document, one of them on a plane. Nothing is lost, neither is surprised, and neither had to understand the merge algorithm.

## Non-goals

_To be detailed._

## Current state

Genuinely both, in pieces that have not yet been unified into one story:

- Vault: plain markdown on local disk, a debounced FS watcher picking up outside edits, sha-based `IfMatch` conflict detection, and per-file Loro CRDT collab armed for the focused pane.
- Files: content-addressed chunks with jj providing op-log concurrency, divergent changes and conflicted trees; a replica sync engine and a daemon with per-device selective sync.
- Transport: architect PubSub streams over vox, with blobs able to ride iroh/QUIC peer-to-peer.

Gaps: no single reconciliation model across those layers, no stated offline-duration limit, and no defined class of data that refuses offline writes.

Related: [A Single Trusted System](02-single-trusted-system.md), [The Ideal Vault (PKM)](08-vault.md), [The Ideal Editor](20-editor.md), [Self-Hostable - Federated Servers - Peer to Peer](22-self-host-federation-p2p.md).
