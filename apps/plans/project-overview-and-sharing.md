# Project overview + client/collaborator sharing

**Status:** designed, not started (2026-08-13)

Make a project page that answers "where is this work and what came out
of it" — deliverables you can play, every related file, the tasks — and
lets it be handed to two different audiences with two different links.

## The driving case

`A Journey of Immigrants — Andres Jaramillio`, on the NAS at
`starcommand:/mnt/storage/Resources/fts-files-fixtures/Dr. Jaramillo's Video 2/`:

```
A Journey of Immigrants - Andres Jaramillio.mp4   9.45 GB   ← video deliverable
Journey of Immigrants - Jaramillio.mp4                      ← a second cut
Audio/                                                      ← audio deliverables
  Ancestro - 5.8.26 Mix.wav                       189 MB
  El Intachable - 5.8.26 Mix.wav                  105 MB
  Etudes for Immigrants - 5.8.26 Mix.wav          391 MB
  Five Colombian Pieces - 5.8.26 Mix.wav          569 MB
  La Zacapaneca - 5.8.26 Mix.wav                  161 MB
Graphics/                    titles + lower thirds (.mov)
Audio Source Files/          the sources behind the mixes
Archive - Original AAC MP4s/ earlier exports, video ISOs
Jaramillio 2.drp             the DaVinci Resolve project
.DS_Store, ._*               macOS litter, must never surface as files
```

Two things fall out of the real data. **Deliverables are a subset, not a
folder** — `Audio/` holds five of them beside `Audio Source Files/` which
holds none, and the top-level `.mp4` is one while the `.drp` beside it is
not. And **9.45 GB will never stream from the original**: the client link
plays renditions or it does not play.

This project is not in Task at all today — it is a folder. So the page
has to work for "a folder someone points at", not only for projects born
in the app.

## What already exists

Most of the parts. This is assembly, not greenfield:

- **File Roots** over existing directories, with the sync daemon and
  per-file status (#264, #265) — how the NAS folder becomes visible
  without moving a byte.
- **Renditions / transcode** (#269) — proxies for exactly the 9.45 GB
  problem.
- **Review** (#270) with the freeframe-parity player, comments, frame
  drawings — already the thing a client would use to respond.
- **Share links** (#271, #272): `ShareTarget::{Note, Slice, NamedVersion,
  Review}`, capabilities `comment` / `download` / `file_request`,
  password, expiry, and a guest lane that boots an anonymous visitor
  into the web app.
- **The org tree** (#304) — `Projects/<name>/…` as one namespace over
  vault folder + File Roots.

## What is missing

1. **A project page worth landing on.** Today `/projects/:id` is a task
   list with metadata. It needs: deliverables (playable), the file tree,
   the tasks, and the share controls in one place.
2. **`ShareTarget::Project`.** Every existing target is one file, one
   slice, or one review. A project link has to resolve to a *set*, and
   the two audiences want different sets.
3. **A "deliverable" concept.** Nothing marks the five mixes and the
   final cut as the outputs, so a client link has nothing to show. This
   is the one genuinely new domain idea here.
4. **Client view mode** — a page with no tasks, no source files, no
   internal comments: cover, deliverables, play, optional download.

## Deliverables

The smallest thing that works: a **`deliverables/` convention plus an
explicit list**. A file is a deliverable if the project note names it —
frontmatter, resolved against the project's roots:

```yaml
deliverables:
  - path: "A Journey of Immigrants - Andres Jaramillio.mp4"
    kind: video
    label: "Final cut"
  - path: "Audio/Etudes for Immigrants - 5.8.26 Mix.wav"
    kind: audio
```

Naming them in the note rather than inferring from a folder is what
handles the real layout: `Audio/` is deliverables, `Audio Source Files/`
is not, and no folder rule distinguishes them. Marking a file as a
deliverable from the file tree writes this list — the UI is the author,
the note is the record, and it survives outside the app like everything
else in the vault.

## The page

```
┌─────────────────────────────────────────────────────────┐
│ A Journey of Immigrants — Andres Jaramillio    [Share ▾]│
│ Andres Jaramillio · active · 6 deliverables             │
├─────────────────────────────────────────────────────────┤
│ DELIVERABLES                                            │
│  ┌───────────┐  Final cut            video   9.4 GB  ▶ │
│  │  poster   │  Etudes for Immigrants audio  6:31   ▶ │
│  └───────────┘  …                                       │
│  (plays inline — rendition, never the original)         │
├─────────────────────────────────────────────────────────┤
│ FILES            │ TASKS                                │
│  the org tree    │  open / done, add                    │
│  rooted at this  │                                      │
│  project         │                                      │
└─────────────────────────────────────────────────────────┘
```

Client mode is the same page with the bottom half gone and the header
reduced to title + client-facing blurb.

## Sharing: two links, one target

`ShareTarget::Project { id, mode }` with two modes, both riding the
existing guest lane and capability axes:

| | Collaborator | Client |
|---|---|---|
| Deliverables | play + comment | play, comment optional |
| File tree | full | hidden |
| Tasks | visible | hidden |
| Downloads | per `download` | per `download`, default off |
| Uploads | per `file_request` | usually off |

Mode is not a capability — it selects *what the link resolves to*, and
capabilities then apply within it. Keeping them separate is what stops
"client link" from becoming a bag of eight booleans nobody can reason
about at 11pm before sending it.

## Staging

**S1 — the folder becomes a project.** Register the NAS directory as a
File Root in the right org, create the project note, point it at the
root. Proves the org tree renders a real 25 GB project and that the
macOS litter stays hidden.

**S2 — deliverables.** Frontmatter list + "mark as deliverable" in the
file tree + the deliverables strip on the project page, playing through
existing renditions. The 9.45 GB file is the acceptance test: if it does
not start in a couple of seconds, the transcode path is not ready.

**S3 — the page.** Files + tasks alongside deliverables; make
`/projects/:id` the place you land.

**S4 — sharing.** `ShareTarget::Project` + mode, the client view, and
the two mint buttons. Reuses the guest lane wholesale.

## Risks

- **Rendition cost.** 9.45 GB in, proxies out, on a NAS reached over the
  network. S2 should measure before promising anyone a link.
- **A client link is the sharpest thing in this app.** It hands an
  anonymous visitor a URL that plays a client's unreleased work.
  Password + expiry exist; the mode must fail CLOSED (unknown mode =
  show nothing) rather than degrade to "everything".
- **`deliverables` is a new vault contract.** Once links depend on it,
  the shape is hard to change — worth getting the field names right in
  S2 rather than after the first client has the URL.
