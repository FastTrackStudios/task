# Project Outputs & Linked Resources Architecture

## The Core Insight

A **Song** is a project. It might live in a Reaper session with 100 tracks, multiple takes, revisions, and stems. But the **setlist** doesn't need any of that — it just needs the finished output: the final mix, maybe the live session file.

This means we need two concepts:

1. **Project** — the full workspace (Reaper session, all takes, stems, mixing notes)
2. **Output** — a finished artifact that other projects can reference (final mix .wav, master .mp3, album artwork .png)

A setlist links to **outputs**, not projects. But outputs link back to their source project for anyone who has permission to drill deeper.

## Output Trait

```
Project: Montreal Album
├── Songs/
│   ├── Track 1 - Overture/
│   │   ├── project.md          ← full project (Reaper session, stems, takes)
│   │   ├── outputs/
│   │   │   ├── v1-rough-mix.wav
│   │   │   ├── v2-mix.wav
│   │   │   ├── v3-final-mix.wav     ← current output
│   │   │   └── outputs.md          ← version history with notes
│   │   └── sessions/
│   │       └── Track 1.rpp
│   └── Track 2 - Daybreak/
│       ├── project.md
│       └── outputs/
│           └── v1-demo.wav
│
├── Setlist/
│   └── setlist.md
│       songs:
│         - title: "Overture"
│           project: "Songs/Track 1 - Overture"     ← link to project
│           output: "outputs/v3-final-mix.wav"       ← link to specific output
│         - title: "Daybreak"  
│           project: "Songs/Track 2 - Daybreak"
│           output: "outputs/v1-demo.wav"
```

## Output Schema

```yaml
# outputs.md — version history for a project's deliverables
---
title: Track 1 - Overture Outputs
project: Songs/Track 1 - Overture
current_version: 3
outputs:
  - version: 1
    file: v1-rough-mix.wav
    date: 2026-03-15
    status: superseded
    notes: "First rough mix, drums too loud"
  - version: 2
    file: v2-mix.wav  
    date: 2026-03-28
    status: superseded
    notes: "Better balance, vocal needs more presence"
    feedback:
      - from: tombrooks
        comment: "Snare sounds great, vocal could come up 1dB"
        timestamp: 2026-03-29
  - version: 3
    file: v3-final-mix.wav
    date: 2026-04-10
    status: approved
    approved_by: codywright
    notes: "Final mix, approved for mastering"
---
```

## Versioning

Nextcloud already provides file versioning natively. Every file edit creates a version in `data/<user>/files_versions/`. But we need **semantic versioning** on top of that — "v1 rough mix" vs "v2 with vocal fixes" vs "v3 final" is different from Nextcloud's automatic file-level versioning.

The `outputs.md` file tracks semantic versions. Nextcloud handles byte-level versioning underneath.

## External Service Integration Points

### Finances

```yaml
# In budget.md or invoice.md
---
finances:
  firefly_iii:
    transaction_id: "12345"
    category: "Music Production"
    budget: "Montreal Album"
  invoice_ninja:
    invoice_id: "INV-2026-042"
    client: "Record Label X"
    status: sent
    amount: 5000.00
    due: 2026-05-15
---
```

The pattern: store the **reference** (ID, URL) in the `.md` file, not the data itself. The external service is the source of truth for financial data. Our system links to it.

### Integration Trait

```rust
/// An external service that a project can link to.
trait ExternalIntegration {
    /// Service identifier (e.g. "firefly-iii", "invoice-ninja", "github")
    fn service_id(&self) -> &str;
    
    /// Sync data from the external service into our model
    async fn pull(&self, reference: &str) -> Result<ExternalData>;
    
    /// Push updates to the external service
    async fn push(&self, reference: &str, data: &ExternalData) -> Result<()>;
    
    /// Generate a link/URL to the resource in the external service
    fn link(&self, reference: &str) -> String;
}
```

### Planned Integrations

| Service | What it provides | How we link |
|---|---|---|
| **Firefly III** | Budgets, transactions, accounts | `firefly_transaction_id` in budget items |
| **Invoice Ninja** | Invoices, quotes, payments | `invoice_ninja_id` in deliverables |
| **GitHub** | Issues, PRs, releases | `github_issue` in tasks |
| **Nextcloud** | Files, versions, sharing | Native (WebDAV) |
| **CalDAV** | Calendar events, scheduling | Native (VTODO) |
| **REAPER** | DAW sessions, markers, regions | `reaper_session` path link |

## Permission Model

When a setlist links to a song's output:

- **Everyone** can see the output file (final mix) and play it
- **Team members** can see the project folder (stems, session files)
- **Project owner** can see everything (takes, notes, rough mixes)

Permissions are inherited from Nextcloud shares:

```yaml
# In project.md
---
permissions:
  public_outputs: true          # Anyone with the link can access outputs/
  team_access: true             # Team members see the full project
  client_portal:                # Client sees only approved outputs
    enabled: true
    client: "Record Label X"
    shared_outputs: ["v3-final-mix.wav", "v3-master.wav"]
---
```

## How This Maps to Samply's Features

| Samply Feature | Our Implementation |
|---|---|
| Project workspaces | Project folders on Nextcloud |
| File versioning | `outputs.md` semantic versions + Nextcloud file versions |
| Audio streaming/playback | Link to Nextcloud file share (streaming) |
| Timestamped comments | Feedback entries in `outputs.md` with timestamps |
| Approval workflows | Output status: draft → review → approved |
| Delivery portals | Nextcloud share links with permission scoping |
| Team roles | PersonnelRole with status (Accepted/Declined) |
| Activity feed | Nextcloud Activity API |
| Invoicing | Invoice Ninja integration via reference IDs |
| Client-facing pages | Nextcloud public shares of output folders |
| Metadata management | YAML frontmatter (ISRC, UPC, credits in song.md) |
| Release planning | Deliverables with due dates and status tracking |

## The Key Difference

Samply is a hosted SaaS. Our system is:

- **Self-hosted** on your Nextcloud
- **File-based** — everything is a `.md` file you own
- **Open** — no vendor lock-in, works with any tool
- **Integrated** — same system for tasks, projects, events, finances, and music production

## Project Scaling: Song → Album → Video → Tour

Projects nest naturally. A song becomes part of an album, the album spawns
music videos, the videos feed into a tour. At each level:

- The child project keeps its own `project.md`, `tasks/`, `outputs/`
- The parent project links to children via `ProjectLink` references
- Shared resources (setlist, stage plot) live in a `shared/` folder
- Per-instance overrides (this venue's specific timing) live in `overrides/`

### Inheritance Model

```
Tour (shared/)
  └── setlist.md          ← default for all dates
      ↓ inherits
  Date (overrides/)
      └── setlist.md      ← "swap song 3 for acoustic version"
```

A date without an override uses the shared version. A date with an override
merges the changes on top.

### Performance Layer

At scale (30 tour dates × 10 songs × crew across cities), filesystem scanning
is too slow for interactive queries. The performance layer:

- **SQLite** indexes all frontmatter fields from `.md` files
- **Redis** caches active project data for sub-millisecond reads
- **File watcher** invalidates cache on `.md` file changes
- **Full rebuild** from files at any time — the cache is disposable

The markdown files remain the source of truth. Always portable, always readable
by any tool, never locked into a database.
