# The example studio

A studio's disk, small enough to commit. Every *shape* here was read off a
real 6 TB archive; the names are invented, so that a test failing on
"Track Two" tells you which case broke without you having to know whose
album it was.

Not `Vault/`, because a vault is one of the things an org *has* — beside
its wiki, its assets, its inbox and its projects — and naming the whole
tree after one part made the other four look like exceptions.

## Who reads this

- `tests/integration/tests/studio.rs` runs the tree reader over it, which
  is what it was first committed for.
- `task-server admin demo` plants it as real orgs on real servers.
  `apps/server/src/example_org.rs` owns the translation from this layout
  to an org root's, and explains why the translation is there rather than
  here.
- The integration suite's scenario boots from it, so the world the
  chapters assert against and the world you can sign into are one world.

It lived under `tests/integration/Data/` while the first of those was the
only one.

```
studio/
  <org>/                             one org, and everything it has
    Assets/                          its library — content owned by no project
    Inbox/                           arrived, not filed into a project yet
    Vault/                           its notes — what it is doing
    Wiki/                            what it knows
    Projects/
      <Work> - <Client>/             a project
        project.md                   what it says about itself, when it says
        <Session>/                   a DAW session — the adoptable root
        Inbox/                       unfiled, this project's
```

The org is the top level, so an org's whole disk is one subtree. That is
what makes it the unit everything else already works on: a host admits an
org, a peer replicates one, a server's data root holds `orgs/<slug>/`.

## The cases, and what each is for

| folder | the case |
|---|---|
| `acme-audio/Projects/First Single - Example Client/` | the ordinary one. One org, one client, one session |
| `acme-audio/Projects/Example Album/` | **several sessions in one project** — three tracks, each its own REAPER session with `Audio Files/` and `Renders/`. And **no `project.md`**, because most of a real archive has none: 14 of 37 |
| `vnt-video/Projects/Example Documentary - …/` | a **video project is its own session** — the `.drp` sits at project level, not in a subfolder. Two of them, versioned by filename. Material lives in folders one person named, including `Archive - Original Camera Files`, which contains ` - `. Two clients, comma-separated |
| `vnt-video/Projects/Shared Project/` | **two orgs on one project**, on the disk of neither the one that started it. See its `project.md` |
| `acme-audio/Projects/Z - Duplicates/` | housekeeping. Contains ` - `, so anything splitting on the dash before checking the prefix reads it as the project "Z" for the client "Duplicates" |
| `acme-audio/Projects/tasks/` | all lowercase where people capitalise — Task's own storage, not somebody's work |
| `*.rpp-bak`, `*.rpp-bak-UNDO` | a DAW's own saves, beside the session file. Not sessions |
| `.DS_Store`, `._Bass.wav` | platform droppings. `files.ignore.layers` says these never reach a listing |

## On `Inbox` rather than `Z - Inbox`

Archives spell it `Z - Inbox`, to keep unfiled material at the bottom of
an alphabetical *file listing*. Nothing browses folders alphabetically any
more — the tree a person sees is built from the catalogue — so the prefix
buys nothing here.

The model reads both, and has to: a studio does not rename six thousand
folders because software prefers a different spelling.

## What is deliberately absent

Large files. `scale.rs` generates those at run time: a fixture that has to
be megabytes to be interesting does not belong in git, and one that has to
be 800 GB cannot be. The tests for chunk boundaries, dedup and resumable
transfer make their own content and throw it away.

## The real archive

`archive.rs` reads whatever `TASK_ARCHIVE_ROOT` names, and is skipped
otherwise — so a checkout without one still runs the whole suite.

That archive is in the **older layout**: orgs under a single top-level
`Projects/`, each org's projects directly inside it, no per-org `Vault/`
or `Wiki/`. The model knows one shape and `archive::org_roots` knows
where to start in either. Keeping the translation in the harness is
deliberate — a model that knew both would carry the migration around
forever.

Against 6 TB it currently reads 6 orgs, 37 projects, 37 inboxes, 334
material folders and 78 sessions, with **nothing unexplained**. Every
awkward case above was found there first, as a name that broke the
reader.
