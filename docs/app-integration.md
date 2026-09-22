# Integrating an app with Task

For an agent building Session, Signal, Ignition, Keyflow — any app that keeps
a musician's work in Task so it is there on every machine they sign in to.
Decisions behind this: ADR 0003 (Task is the shared backend), ADR 0004
(assets, resources), ADR 0005 (the Files lanes as the apps' store).

## The model

**The split.** Every thing an app keeps has two halves, stored in two places:

- **The manifest** says what the thing *is*: a small document with a title,
  tags, the app's own settings, and a `ContentRef` pointing at its bytes.
  Patches, samples and lighting documents are manifests in the resources lane;
  charts and songs are collaborative vault documents. Manifests are cheap to
  list, search, subscribe to and sync to a phone.
- **The bytes** are what it *weighs*: audio, impulse responses, DAW session
  files, video. They live in a **File Root** — a versioned folder tree the
  Files lanes serve — and are fetched on demand, by range when large.

A library is a **collection** — an ordered list of node references
(`sample:room-kick`, `song:hosanna`) with a kind string the app chooses. A
setlist, a sample library and a rig's patch bank are all collections.

Everything belongs to an **org**. Every account has a personal org; bands and
churches are orgs with several members. An app never has a store of its own:
it is an OIDC client of Task, like Task's own web app, and reaches the org
over vox.

## Steps

Work through these in order. The integration is done when the checklist at
the end holds.

### 1. Depend on the wire crates

From `https://github.com/FastTrackStudios/task` (git, `branch = "main"`):

| crate | for |
|---|---|
| `task-dial` | the authenticated dial, browser and native |
| `files-client` | the Files lanes the way an app uses them |
| `files-proto` | Files types (`RootId`, `RootPath`, `FilesEvent`, `FilesFault`) |
| `resources-proto` | manifests: `PatchDoc`, `SampleDoc`, `LightingDoc`, `ChartDoc`, `SongDoc`, `ContentRef` |
| `collection-proto` | libraries, setlists: `CollectionServiceClient` |
| `links-proto` | `NodeRef` — the address of a thing |
| `org-proto` | `OrgManagementServiceClient` (step 3) |

All of them build for `wasm32-unknown-unknown`; the gate checks `task-dial`
and `files-client`. A native app that wants Task in process rather than over a
socket uses `task-client` instead of `task-dial` (native only, heavier).

### 2. Sign in

The app is its own OIDC client of the central issuer (fts-auth). Register the
client and **every scope it requests** in
`~/.starcommand/modules/services/fts-auth/fts-auth.nix` (`oidcClients`), then
`just gitops-push`. A scope the registration lacks is a 403 at
`/oauth2/authorize` — that is the usual cause of a sign-in that "just
breaks". Sign-in yields an access token; Task resolves it to the same person
whichever app issued it.

### 3. Find the person's org

On first sign-in a person belongs to no org and every lane refuses them. Ask
the server for their personal org — idempotent, so call it on every sign-in:

```rust
let orgs: OrgManagementServiceClient =
    task_dial::establish_at("wss://task.fasttrackstudio.app/server/vox", Some(&token)).await?;
let org = orgs
    .ensure_personal_org(PersonalOrgRequest { session_token: token.clone() })
    .await?;
// org.slug — the org to dial. Let the person switch to a band's org later.
```

### 4. Dial the org

One connection per typed client (the token rides each handshake):

```rust
let url = format!("wss://task.fasttrackstudio.app/org/{}/vox", org.slug);
let dial = |u: &str| task_dial::establish_at(u, Some(&token));
let files = files_client::FilesClient::new(
    dial(&url).await?, dial(&url).await?, dial(&url).await?,
    dial(&url).await?, dial(&url).await?,
);
let resources: ResourcesServiceClient = dial(&url).await?;
let collections: CollectionServiceClient = dial(&url).await?;
let stream: TreeServiceStreamClient = dial(&url).await?; // live events, step 10
```

Keep these for the session and redial on a dead connection — `task-dial`
leaves that policy to the app.

### 5. Make the app's stores

One root per kind of bytes, named `<app>/<kind>`, asked for on every start:

```rust
let irs = files
    .ensure_root("signal/impulse-responses", "Impulse responses", RootFlavor::Media)
    .await?;
let irs = files_client::root_id(&irs);
```

`ensure_root` returns the existing root when there is one. `RootFlavor::Media`
for audio, video and anything large; `RootFlavor::Software` only for trees
that are really source code (git-backed, no chunk store).

### 6. Save bytes

```rust
let saved = files
    .put(irs, "Cab/4x12 V30.wav", &wav_bytes, Save::create_only())
    .await?;
let etag = saved.content.clone().expect("a landed file has an etag");
```

`put_reader(root, path, size, reader, save)` streams a file that should not
be held in memory. Keep the **etag** with whatever the app shows as "the
file", and pass it back on the next save — see *Saving safely* below.

### 7. Declare the manifest

Bind the manifest to the bytes, pinned to exactly the bytes just saved:

```rust
let content = ContentRef {
    root_id: irs.to_string(),
    path: "Cab/4x12 V30.wav".into(),
    content: etag.0.clone(),
};
let made = resources
    .upsert_sample(SampleDoc {
        slug: String::new(), // server derives it from the title on create
        title: "4x12 V30".into(),
        tags: vec!["ir".into(), "cab".into()],
        duration_secs: 1,
        sample_rate: 48_000,
        body: String::new(),
        content,
        updated_at: now_rfc3339(),
    })
    .await?;
// made.slug — the node id: `sample:<slug>`
```

`upsert_patch` / `upsert_lighting` take the same shape. The manifest's own
fields are the app's; the server stores what it is given.

### 8. Gather into libraries

```rust
let lib = collections
    .create(org.slug.clone(), "Cabinet IRs".into(), CollectionKind::from("ir-library"))
    .await?;
collections
    .add_item(Placement {
        collection_id: lib.id.clone(),
        node: NodeRef::parse(&format!("sample:{}", made.slug)).unwrap(),
        after: None,
    })
    .await?;
```

The kind string is the app's to define and document (`setlist`, `songlist`,
`ir-library`, `patch-bank`); Task only orders the items and resolves them. An
item may name another org's node (`guest.example/song:hosanna`) — it resolves
only if the reader can already reach that org.

### 9. Read back

```rust
match files.resolve(root, &path, pinned.clone()).await? {
    Resolved::Current(entry) => { /* the path still holds the pinned bytes */ }
    Resolved::Moved { .. } => { /* someone replaced it; get_ref still yields the pinned bytes */ }
    Resolved::Missing { .. } => { /* gone from the path */ }
}
let bytes = files.get_ref(root, &path, pinned).await?; // exact bytes the manifest named
let head = files.read_range(root, &path, 0, 65_535).await?; // a seek, not a download
files.read_to(root, &path, |chunk| sink.write(chunk)).await?; // stream, hold nothing
```

`get` holds the whole file; use it only for small things.

### 10. Follow changes

Subscribe first, then read current state, then fold events in — so nothing is
missed between the two:

```rust
FilesClient::events(&stream, Some(root), |event| {
    match event {
        FilesEvent::Upload(UploadEvent::Completed(entry)) => refresh(entry.path),
        FilesEvent::Tree(TreeEvent::Changed(delta)) => apply(delta),
        FilesEvent::Version(VersionEvent::Checkpointed(_)) => mark_saved(),
        _ => {}
    }
    true // keep listening
})
.await?;
```

Events arrive filtered to what the person may read. A subscriber that falls
behind catches up with `TreeServiceClient::changes_since(root, cursor)`; it
never re-lists the tree.

### Done when

- [ ] A fresh account signs in and lands in its personal org with no manual step.
- [ ] The app's roots are found by `ensure_root` on every start, never created twice.
- [ ] Every save passes an `Expect` (`create_only` or `replacing(etag)`) unless last-writer-wins is genuinely what the person wants.
- [ ] Every manifest that names bytes carries a pinned `ContentRef`.
- [ ] A second machine signed in as the same person lists the manifests and fetches the bytes.
- [ ] A late save from a second machine surfaces `Stale` to the person instead of overwriting.
- [ ] The app's collection kinds are written down in the app's own docs.
- [ ] Large files move with `put_reader` / `read_to` / `read_range`, never whole in memory.

## Per-app recipes

**Signal (rigs, patches, sample libraries).** A patch is a `PatchDoc` whose
`body` is the rig's settings verbatim and whose `rig` names the device
(`helix`, `kemper`). Its impulse responses and samples are `SampleDoc`s over
bytes in `signal/impulse-responses` and `signal/samples`; the patch refers to
them by `sample:<slug>` in its body. A rig's patch bank is a collection of
`patch:` nodes. Attach a patch to a song by reference (`song:<slug>`), never
by copying.

**Session (tracks, DAW sessions).** A project's sessions, takes, stems and
renders are bytes in a root per project (`session/<project-slug>`), a Media
root. Save the session file with `Save::replacing(etag)` — this is the case
safe saving exists for. Name a milestone with `CurationServiceClient::
name_version` so it survives retention. A DAW opens files on local disk, so
the working copy on a machine comes from the Files daemon, not from streaming
— see *Not built yet*.

**Keyflow (charts).** Charts are vault documents, not bytes:
`ResourcesServiceClient::upsert_chart` / `chart` / `list_charts`, with
collaborative editing. Song lists are collections of kind `songlist` over
`song:` nodes. Attachments a chart needs (a reference recording) are bytes in
a root, referenced by `ContentRef`.

**Ignition (lighting).** A show is a `LightingDoc` whose `cues` are the
anchors `lighting:<slug>#cue:<label>` addresses; the console's show file is
bytes in `ignition/shows`, bound by `ContentRef`.

## Reference

### Saving safely

| `Save` | lands when | use for |
|---|---|---|
| `Save::create_only()` | nothing is at the path | new files |
| `Save::replacing(etag)` | the path still holds `etag` | an edited file — the default for anything a person edits |
| `Save::overwrite()` | always; old content stays in history | generated output (renders, caches) |
| `Save::keep_both()` | always, beside any occupant | imports |

A refused save is `ClientError::is_stale()` — show the person both versions
and let them choose; re-fetch the current etag with `files.entry(root, path)`.
The etag is issued by the server (`CatalogueEntry::content`); an app never
computes one.

### Access

What a person may do is their org role united with any grants: owner, admin
and member reach every root; other roles read. A person with no membership
row holds only what was granted to them (a client given one `Deliverables`
folder). Share a folder with `AccessServiceClient::grant`. Refusals read as
absent (`PathNotFound`, `RootNotFound`), never as "forbidden", so a hidden
path cannot be discovered.

### Faults

Errors are `ClientError::Fault(FilesFault)` — the domain answer — or
`Transport` / `Stream`. The ones an app handles:

| fault | meaning | do |
|---|---|---|
| `Stale` | the file moved on since the etag | show both, let the person choose |
| `Exists` | `create_only` found something | offer `keep_both` or `replacing` |
| `PathNotFound` / `RootNotFound` | absent, or not theirs to see | treat as missing |
| `Denied` | known, and not permitted | explain which capability is missing |
| `Unavailable` | bytes held somewhere unreachable right now | retry later; the entry is real |

### Ids and paths

`RootId` wraps a UUID; `RootPath` is root-relative with `/` separators and no
`..` (`RootPath::parse`). A `ContentRef` stores both as strings, so a manifest
stays readable by anything. A version is a `VersionId` built from a **full**
commit id (`VersionId::from_commit_hex`); a short prefix a person types must
be expanded against `VersionService::chain` first.

## Not built yet

Plan around these; do not search for them:

- **The desktop working copy.** Files the DAW opens come from the Files
  daemon (`fts-files-daemon`), which syncs roots to disk with on-demand stubs.
  A device's identity still resets when the daemon restarts, and "a session
  brings its media with it" is specified but not wired — Session needs both.
- **A browser playing an original.** Bytes ride vox; a browser `<audio>` tag
  reaches only renditions (proxies, waveform peaks) through the rendition
  route. Play originals through `read_to` into Media Source Extensions.
- **Versioned manifests.** Patch, sample and lighting manifests are plain
  files on the server, not versioned and not on the live stream. Charts and
  songs are.
- **Server-side homes for Signal and Ignition.** The manifest kinds exist; no
  app-specific server code does.

The current list of gaps is `docs/spec/unmet.md`; the full surface is
`docs/api-reference.md` (generated from the permit registry).

## Where the code is

| what | where |
|---|---|
| the client | `features/files/files-client/src/lib.rs` |
| Files lanes (wire) | `features/files/files-proto/src/service/` — one trait per spec section |
| Files rules | `features/files/spec/files.md` |
| manifests | `features/resources/resources-proto/src/lib.rs` |
| collections | `features/collection/collection-proto/src/` |
| the end-to-end chapter an app should mirror | `tests/integration/tests/it/app_store.rs` |
| the planted example (a sample bound and pinned) | `apps/server/src/demo_cli.rs` → `plant_bound_sample` |
