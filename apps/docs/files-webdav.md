# Files over WebDAV

The **WebDAV bridge** (issue #274) lets any OS file manager mount an
org's File Roots without the sync daemon installed. It is a *compat
bridge*, not the sync path: it shows the current head of each root's
live tree, read-write, and nothing else.

- **Endpoint**: `https://<task-server>/org/<slug>/dav/`
- **What you see**: one folder per WebDAV-visible File Root; inside
  each, that root's live tree as it is right now.
- **What you do not see**: version history. A root's marker file and
  version store (`.fts-root.json`, `.fts-files`) do not exist as far as
  WebDAV is concerned — they cannot be listed, read, written, or
  deleted through the mount. Version chains live on the Files RPC
  surface (`FilesService::chain`), not here.

Writes are ordinary writes: a file dropped in through Finder lands in
the live tree and is picked up by the next scan-certified Session
checkpoint, exactly like a DAW saving over NFS.

## Mounting

The mount always authenticates. Use your Task account's email as the
username, and either your password or a Task session token as the
password. A password is verified against the store on every request —
no session is created, so a rotated password takes effect immediately
and a left-up mount does not accumulate sessions.

**macOS (Finder)** — ⌘K, then:

```
https://<task-server>/org/<slug>/dav/
```

**Windows (Explorer)** — *Map network drive…* → *Connect to a Web site*,
same URL. Windows requires HTTPS for Basic auth by default.

**Linux (GNOME Files)** — *Other Locations* → *Connect to Server*:

```
davs://<task-server>/org/<slug>/dav/
```

**Command line** — a session bearer works too, which is handy for
scripts:

```bash
curl -X PROPFIND -H 'Depth: 1' \
  -H "Authorization: Bearer $TASK_SESSION_TOKEN" \
  https://<task-server>/org/<slug>/dav/
```

## Hiding a root from WebDAV

On the server:

```bash
# what's exposed right now
task-server admin webdav --org <slug>

# hide / un-hide one root
task-server admin webdav --org <slug> --hide <root-id>
task-server admin webdav --org <slug> --show <root-id>
```

Like every other `admin` verb, authorization is filesystem ownership of
the data root — in the cluster, `kubectl exec` into the pod.

Underneath, this is a policy file next to the org's Files registry,
which an operator can also edit directly:

```
<data_root>/orgs/<slug>/files/webdav-policy.json
```

```json
{
  "hidden": ["3f1c…-…-…", "9ab2…-…-…"]
}
```

Listed roots vanish from the mount — not listed, not reachable by name
or by uuid, indistinguishable from a root that was never created. They
stay fully usable over the Files RPC surface. The file is re-read when
its mtime changes, so an edit takes effect on the next request with no
restart; a malformed file is ignored (the previous policy stays in
force) rather than silently un-hiding anything.

## Notes and limits

- **Folder names.** A root appears under its name. Two roots with the
  same name are both suffixed with a short id, so a folder name never
  depends on creation order. A root is also always addressable by its
  uuid, which is the stable form for scripts.
- **The mount point is read-only.** Creating a folder at the top level
  does not create a File Root — roots are created through
  `FilesService::create_root`, which mints the id, writes the marker
  and initializes the version store.
- **Cross-root moves are not supported.** Each root is served as its
  own WebDAV namespace; a `MOVE` between two roots is refused. Within a
  root, moves and copies work normally.
- **A root itself cannot be deleted or moved through the mount.**
  Dragging a mounted root folder to the Trash is refused with `403`.
  A File Root has an identity, a marker and a version store; it is
  removed through Files, not by a drag. Anything *inside* a root
  deletes normally.
- **This is not the sync path.** For a real local replica with offline
  versioning, use the sync daemon. WebDAV is for the machine that does
  not have it.
- **Access is org-wide, then per-root visibility.** Any member of the
  org reaches every root the policy does not hide. Narrowing further —
  root granularity narrowed by slices, per the Files spec — waits on
  the Files permission model; until then, `--hide` is the only
  per-root control.
- **No signed-URL grants.** `/media`'s `?token=` grants are scoped to
  paths under the org's `resources/` tree, a different namespace from
  the root segments here; honouring one on this route would widen it.
  Use a session bearer or Basic auth.
