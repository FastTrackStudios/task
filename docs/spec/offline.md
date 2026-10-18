# Offline Spec

A musician opens Task on a train, in a basement studio, on a plane. The app
opens, shows everything they had, lets them capture, process, tick things off
and write — and when the connection comes back, all of it reaches the server
and every other device, with nothing lost and nothing silently overwritten.

That is the promise in `VISION.md` ("every client has a full copy … works
offline, syncs when connected"). Until this spec, the app was online-only:
optimistic writes rolled back when the server was unreachable, nothing was
cached across launches, and an offline cold start ended at the sign-in form.

Cross-cutting: every client (web, desktop, iOS), the shared stores
(`crates/stores`, `crates/ui/src/stores.rs`) and the server's vault and
collab layers.

## The decision (2026-10-13)

**Loro documents are the truth, and their history is the version control;
plain markdown is how things come in and go out.** Every authored thing — a
note, a task, an inbox capture, a project, a day plan — is a **Loro document**
(a text body plus a map of its properties), kept in a **Turso database** on
every device and on the server, with an index of rows. Every change, offline
ones included, is in the document's history, with who and when, and any
earlier version can be rebuilt. Plain text is the interface, not the store:

- **In** — the desktop vault folder (an AI, Obsidian or a script writing
  markdown there, offline too), imports, and *sources*: a git repository's
  docs mirrored in (the repo-sourced wiki, `wiki_proto::RepoSource`), where the
  repo stays the truth for its own files and accepted changes go back to it as
  Edit Requests (pull requests).
- **Out** — the desktop folder (always current), "Export vault", and the
  server's markdown mirror, kept so the CLI, file-reading agents and backups
  see plain files.

An edit that arrives as text (the folder, an import, a source pull) is read
into its document as a change of its own; the files are always written from
the documents.

What Loro history is not: it has no branches or reviewed merges (repo sources
keep those in git), and a whole-vault restore needs a vault-level checkpoint
— the documents' versions recorded together — which is its own rule below.
History grows with every edit, so documents are compacted periodically,
keeping the history.

This replaces, for the vault, the earlier rule that markdown files are the
source of truth (`VISION.md` §1, `docs/spec/storage.md` "Authored state is
markdown"): ownership is now guaranteed by the always-current folder and
export rather than by the file being the record. Those documents and ADR 0001
(jj/CAS for the vault) need updating; File Roots for media keep jj/CAS.

| Layer | What it holds | Where |
|---|---|---|
| **Documents** | one Loro document per vault file — `body` (text) + `props` (map of frontmatter fields) — for notes, tasks, captures, projects, plans | device: the local Turso database (doc snapshots + pending updates); server: vault-collab's registry |
| **Index** | queryable rows projected from the documents (the lists every page shows) | device: tables in the same Turso database; server: the existing projections |
| **Pending** | documents created or changed here whose updates the server hasn't acknowledged | the same database |
| **Files** | the markdown itself, versioned | server vault (and the desktop Files mirror) |

This is how a file is **created and updated completely offline**: a new note,
task or capture is a new document with its path, written to the local
database at once and listed from the index; edits are Loro updates; when the
connection returns the documents sync and the server writes (or rewrites) the
files. Nothing waits for the server to exist.

Turso is the **local engine only** — its own sync is not used (it replicates
whole databases and pages, which would bypass per-file permissions and make a
second source of truth beside the files). Sync is Loro over our vox
transport, file by file, under the server's existing access checks. Where
Turso can't run (a target it doesn't build for), the same storage API falls
back to IndexedDB on web.

Records that are *computed*, not authored — activity, notifications, timer
sessions, members — are not documents: the device keeps their last rows in
the index for offline reading, and timer start/stop made offline queue as
pending service calls (`offline.outbox`).

---

## Opening offline

### The app opens to the workspace without the server

t[offline.boot]
With a signed-in account remembered on the device, the app opens to the
workspace when the server cannot be reached: the last account, its orgs and
every store's local replica are read at boot, and the shell shows them with a
quiet "Offline — changes will sync" state instead of the sign-in form. Signing
in for the first time still needs the server.

---

### Every list is served from the device's index

t[offline.replica]
Every list the app shows (tasks, inbox, projects, milestones, goals, plans,
templates, timer sessions, notifications, activity, members) reads from the
device's local Turso index first, and the index is kept current from the
documents and from the server's streams. With no connection the lists are the
ones the person last had, marked stale, never empty or errored.

---

## Every authored thing is a document

### One Loro document per vault file

t[offline.doc.shape]
Each authored vault file — note, task, inbox capture, project, day plan — has a
Loro document: `body`, a text, and `props`, a map of its frontmatter fields
(nested values as maps and lists). The app reads and writes the document, never
the file; the index rows are projections of the documents' `props`.

---

### Documents persist on the device

t[offline.doc.replica]
Documents live in the device's Turso database (a snapshot and the updates since
it). Edits with no connection go into the local document; when the connection
returns it syncs with the server's document, text merging character by
character and each property merging by key — so an offline status change and
someone else's online title change both survive, and two people writing the
same note offline both keep their words.

---

### Files are created and changed fully offline

t[offline.doc.create]
Creating a note, a task, a capture or a project with no connection makes its
document at once, under the path the server will give the file (chosen on the
device, collision-safe), and it appears in every list from the index. Edits
change it as freely as any other document. Nothing about creating or editing
waits for the server; the file appears in the vault when the document first
syncs.

---

### Files are written from documents, and text coming in becomes a change

t[offline.doc.materialise]
Markdown is always produced from a document — YAML frontmatter from `props`,
the body from `body` — by one conversion used by the server's mirror, the
desktop folder and export. Text arriving from outside (a file edited in the
folder, an import, a source pull) is read and merged into its document as a
change of its own, attributed to where it came from. The services that used to
rewrite whole task files (`TaskService::update`) write through the document
instead.

---

### History is the version control

t[offline.doc.history]
Every document keeps its full history — each change with who and when, made
online or off — and any earlier version of a document can be shown, compared
with the current one, and restored (restoring is itself a new change, so
nothing is lost). A vault-level checkpoint records every document's version at
a moment (taken on demand and periodically), so the whole vault can be shown
or restored as it was then. Documents are compacted from time to time without
losing that history.

---

### Plain sources come in, and stay theirs

t[offline.sources]
A git repository's docs (or any plain-markdown source) can be mirrored in as
documents — a repo-sourced wiki (`wiki_proto::RepoSource`) is the first kind —
searchable, linkable and embeddable like any other. The source stays the truth
for its own files: a pull brings its changes in as changes, and an edit made
here goes back as an Edit Request (a pull request), not a silent divergence.

---

### On a desktop, the vault is also a folder that works offline

t[offline.doc.local-folder]
A desktop keeps the org vault as a folder of markdown on disk
(`~/Task/<org>/Vault/`), and the folder is a working surface in its own right,
with or without a connection: a file an AI, a script or another editor creates
or changes there is read into its document in the local database at once (so
the app shows it offline), and edits made in the app are written back into the
folder at once (so the folder always holds the current text). On reconnect the
documents sync with the server (their history records every change) and the
merged result comes back down into the folder. A move or rename in the folder
is recognised as one — by the document id in the file's properties, else by
content — not as a delete and a new file. Each desktop writes only its own
folder; the documents are what sync. Large media and
other File Roots keep the Files sync agent; the vault folder moves to this
path. iOS has no such folder: files there are made in the app (offline too).

---

### Blocks are addressable, embeddable and indexed on the device

t[offline.doc.blocks]
The device's index holds every block of every document — heading, paragraph,
list item, checklist item, code block — with its document, its position and its
anchor if it has one. A block gets an anchor (ADR 0002's `^short-id`, written
inline at its end) when something first references it, not before, so files
stay clean; an anchored block keeps its id through edits, moves and renames
because the document tracks it. `[[Note#^id]]` links to a block and
`![[Note#^id]]` embeds it: the embed renders live from the source document,
editing inside it edits the source, and every block shows where it is
referenced. Picking a block to link or embed (`((` search) is a lookup in the
index. All of it works offline, from the device's own index; the files carry
only the anchors and the `![[…]]` syntax, which Obsidian also reads.

---

### The whole vault exports as plain markdown at any time

t[offline.export]
"Export vault" writes every document the person may read as the markdown
file the server would write — frontmatter from `props`, body from `body`,
block anchors and embeds as text — into a folder or a zip, from the device's
own documents, with or without a connection. It is the same conversion the
server and the desktop folder use, so an export opens as an Obsidian vault and
imports back into Task unchanged.

---

### The app runs the backend itself

t[offline.local]
The app is fully functional with no server: a desktop or iOS install hosts
the backend in-process on the device's own data root (`<data dir>/task/node`)
and every page talks to it through the same typed clients it would send to a
host — routed below the pages (`caller_for`), so no page knows which. Org
discovery is the node's own discovery document; the person is a local
account the node provisions on first run (a home personal org they own, its
credentials kept on the device), signed in through the org's ordinary auth
lane with the session baked into the permission gate as a socket upgrade
bakes it — a device's node is no back door to its own data. It is the
default when no host is configured, and Settings offers it beside "Connect
to a host / another device". A debug build may plant the example studio into
the node instead. When a host cannot be reached, an org the device also holds
a node for is served from the node; an org it does not hold is not replaced
by a different one. A paired Apple Watch reaches a phone's node through the
phone: the phone tells the watch its address is `local://this-device`, and
the watch sends each watch-API call over WatchConnectivity for the phone to
answer from its node, the bearer checked as a host checks it.

---

### Every native install is a node; the server is just one that's always on

t[offline.peers]
Desktop and iOS installs run the backend themselves (`offline.local`) and are
peers of each other: an org's nodes — every admitted device plus any always-on
host — sync its documents with each other directly over iroh, using the
org's existing peer identity and admitted set (`iroh_host`, `device_sync`,
`enroll_device`). There is no hub: two laptops in the same room with no
internet converge with each other, and later with the host, and the host is
only special in that it is usually reachable. Document sync is symmetric —
any node may hold, serve and pull any document its person may read — so the
same Loro exchange runs device↔host and device↔device. The web UI is the one
pure client: it holds no node and talks to whichever node serves it (a host,
or a desktop on the same network). Access is enforced by every node, per
document, against the org's membership — a peer is trusted with what its
person may read, never more.

---

### Access is checked per document

t[offline.doc.access]
A device receives and may sync only the documents of files its person may read
(and write, for edits), checked by the server per document on every sync, the
same checks a live write has. A document created offline under a path its
person may not write is refused on first sync and stays local, marked, never
silently dropped.

---

## Computed records

### What isn't authored is cached, and offline service calls queue

t[offline.outbox]
Records the server computes rather than a person authors (activity,
notifications, members, timer sessions) are read offline from the index. The few
service calls on them a person makes offline — starting and stopping a timer or
a mode — go into a durable queue, replayed in order through the same service on
reconnect; a refusal drops the entry with a notification naming it.

---

## Saying what is true

### The person can see what has not synced

t[offline.visible]
While any document has updates the server hasn't acknowledged, or any queued
call is waiting, the shell says how many changes are waiting, and rows and
editors with unsynced changes show it; the state clears when they sync. The
app never claims a change synced that has not.

---

## Where each rule is checked

| rule | checked by |
|---|---|
| `offline.replica` | unit tests over the local store (index from documents, stale-not-empty) |
| `offline.doc.shape`, `offline.doc.materialise` | unit tests (doc ⇄ markdown round trip, outside-edit merge); e2e: a file edited on disk merges into the open document. Today: `features/vault/vault-doc` unit tests (`markdown`, `doc`), `vault-collab` unit tests (props merge by key, whole-file conversion, a closed-doc edit merged with unflushed doc edits), `apps/server/tests/it/offline_doc_e2e.rs::a_file_edited_on_disk_merges_into_the_document` |
| `offline.doc.replica`, `offline.doc.create` | e2e over the collab transport: documents created and edited on two disconnected replicas converge; the server writes the files. Today: `offline_doc_e2e::two_replicas_edited_apart_converge_and_the_server_writes_the_file` (one replica restarted from storage in between), `offline_doc_e2e::a_document_created_offline_gets_its_file`; unit: `vault_doc::replica` (reopen keeps unsynced edits) |
| `offline.doc.local-folder` | e2e: with the server unreachable, a file written into the folder appears in the local index, an app edit appears in the file, a rename keeps its document; on reconnect both reach the server |
| `offline.doc.blocks` | unit tests (block parse, anchor assigned on first reference, id stable across edit and move); e2e: an embed updates when its source block is edited, offline and after sync |
| `offline.doc.history` | unit tests (restore is a new change; checkpoint restores every document's version); e2e: an offline edit appears in history with its author after sync |
| `offline.sources` | e2e: a repo-sourced wiki pull lands as changes; a local edit becomes an Edit Request |
| `offline.export` | e2e: export from the device's documents with the server unreachable; re-import yields identical documents |
| `offline.local` | e2e (`apps/server/tests/it/local_node_e2e.rs`): a fresh node on a temp root with no listener provisions its person, who signs in and drives task create/update, inbox capture + process, the Suggested pick and the activity feed; a reboot keeps both. `a_watch_reaches_the_phones_node_through_the_phone` — the watch API answered in-process (the phone's relay), an unsigned call refused. Unit: `task_ui_core::node` routing table (local vs remote, unreachable fallback only for a held org) |
| `offline.peers` | e2e: two embedded nodes with no host converge a note edited on both, then a third (the host) catches up from either. Today: `peer_docs_e2e` — two device nodes, no host, dialling each other over iroh: a note written on one arrives on the other as a file, a task made on the other shows in the first's task list, and a note edited on both while apart converges to one text holding both edits on both (the `DocPeer` lane + `doc_peer::pull_peer` in the device sweep); `local_node_e2e` — the join handshake; `vault-doc` unit — the same file seeds the same document on every node. Not yet: the third-node catch-up, per-person access (a peer is an org device, trusted with the org's documents) |
| `offline.doc.access` | e2e: a document for a path the person can't write is refused and kept local. Today: `offline_doc_e2e::refused_documents_stay_on_the_device` (foreign lineage, unadmitted path); the per-person write refusal itself is `wiki_edits` (`open_collab` refused for a non-Editor) |
| `offline.outbox` | unit tests over a fake transport; e2e: timer started offline lands on reconnect |
| `offline.boot`, `offline.visible` | iOS simulator check with the demo server stopped (manual until a UI harness exists) |

## Order of work

1. **Device storage** — the Turso-backed local store (documents, index,
   queue) behind one API, with the web fallback.
2. **Notes** — persisted documents and offline create/edit for vault notes
   (the collab path that exists today, made durable).
3. **Tasks and captures as documents** — `props` + `body`, the server
   materialising task files, `TaskService` writing through documents; the
   lists served from the index.
4. **Projects, plans** — the same, then the rest of the authored kinds.
5. **Boot and visible state** across all of it.
6. **The desktop vault folder** — watcher in, local writer out
   (`offline.doc.local-folder`), replacing the Files agent for the vault root.
7. **Blocks** — the block index, anchors on first reference, `((` search,
   live embeds and backlinks (`offline.doc.blocks`).
8. **Export vault** (`offline.export`).
9. **History** — document history views and restore, vault checkpoints,
   compaction (`offline.doc.history`); then **sources** beyond repo-sourced
   wikis (`offline.sources`).
10. **Retire file-truth** — update `VISION.md` §1, `docs/spec/storage.md`
    ("Authored state is markdown") and ADR 0001 to this decision (write an ADR
    0006 for it); the server keeps a markdown mirror.

## Seeing it in the demo

1. `just demo serve` and `just demo desktop` (or `just mobile-sim`); let the
   app sign in and open Tasks, Inbox and the Schedule once — that writes
   their device copy to `<data dir>/device.db`.
2. Stop the demo server (kill its recorded PID; not `pkill`).
3. Relaunch the app: it opens to the workspace as the same person, the
   lists are the ones last seen, and the pill says "Offline — changes will
   sync" (`offline.boot`, `offline.replica`).
4. Start and stop a timer: the clock changes at once and the pill counts
   "2 changes waiting to sync" (`offline.outbox`, `offline.visible`).
5. `just demo serve` again: on reconnect the pill shows "Syncing…", the
   calls land in order, and the count clears (`offline.replay`).

With no server at all (`offline.local`):

1. Launch the desktop app with nothing configured — no `TASK_VOX_URL`, no
   host selected (`just demo local`, which isolates the app's data dir so
   a real registry cannot win). A debug build plants the example studio
   into `<data dir>/task/node` on first boot (`TASK_NODE_EXAMPLE=0` makes
   a personal org instead) and signs in as its owner.
2. Everything works with no process on :18080: tasks, inbox, projects,
   the Schedule. Settings → Servers shows "This device ● running here",
   with the node's endpoint id per org.
3. To join a host's org: select the host, sign in, and press "Keep on
   this device" beside the org — the device takes a node of it and the
   host enrolls it (`offline.peers`).
