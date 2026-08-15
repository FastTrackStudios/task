# Ecosystem Research — Crates, Tools, and Architecture References

## Recommended Stack for Task

| Layer | Crate | Version | Why |
|---|---|---|---|
| **CRDT (metadata)** | `automerge` | 0.8.0 | JSON-like CRDT for field-level task/event metadata sync. Rust-native. |
| **CRDT (rich text)** | `yrs` | 0.25.0 | Yjs port for markdown body collaborative editing. Proven at scale (AppFlowy, AFFiNE). |
| **WebSocket sync** | `yrs-axum` | 0.8.2 | Yrs sync protocol over Axum WebSocket. Drop-in for our existing Axum server. |
| **WebSocket transport** | `tokio-tungstenite` | 0.29.0 | Standard async WebSocket. Already in our dep tree via Axum. |
| **SQLite index** | `rusqlite` | 0.39.0 | Synchronous SQLite for indexing .md frontmatter. Mature, fast. |
| **SQLite migrations** | `rusqlite_migration` | 2.5.0 | Schema evolution for the index cache. |
| **File watching** | `notify` | 7.x (current) | Already using this. Cross-platform, solid. |
| **CalDAV client** | `libdav` | 0.10.3 | Already using this. Unified CalDAV+CardDAV. |
| **CalDAV (fast)** | `fast-dav-rs` | 0.4.2 | Alternative — HTTP/2, compression, batching. Consider for high-volume sync. |
| **iCalendar** | `icalendar` | 0.17.x | Already using this. VTODO round-trip. |

## CRDT Libraries (Detailed Comparison)

### Tier 1 — Production Ready

| Project | Stars | Focus | Best For |
|---|---|---|---|
| **automerge** | ~6,100 | JSON-like CRDT, full data model | Structured metadata (task fields, event properties) |
| **loro** | ~5,230 | High-perf CRDT, rich text (Peritext) | If we need the fastest possible CRDT with rich text |
| **yrs** | ~1,980 | Yjs port, protocol-compatible with JS | Markdown body editing, JS interop for web client |
| **diamond-types** | ~1,800 | Fastest text CRDT | If pure text performance is critical |
| **cr-sqlite** | ~5,000+ | CRDT-enabled SQLite extension | Our SQLite index could gain CRDT sync for free |
| **crdts** | ~1,400 | CRDT primitive toolkit | Building custom distributed data structures |

### Recommendation: Automerge + Yrs

- **Automerge** for task/event metadata — each task's YAML frontmatter becomes an Automerge document. Field-level concurrent edits merge automatically.
- **Yrs** for markdown body — the text content after `---` is a Yrs text document. Multiple people can edit subtask checklists simultaneously.
- **cr-sqlite** for the index cache — our SQLite index gains CRDT properties, enabling index-level sync between devices without full file re-scan.

This is the same split Plane uses (REST for structured data, Yjs for documents) but fully Rust-native and offline-capable.

## Architecture References

### AppFlowy (closest architecture match)
- **Stack:** Flutter UI + Rust core (like our Dioxus + Rust)
- **Key pattern:** `AppFlowy-Collab` crate wraps Yrs with typed domain API
- **Storage:** Pluggable backends (RocksDB local, Supabase cloud) — validates our `ProjectProvider` trait
- **Lesson:** Build a typed Rust API over Yrs/Automerge mapping to our domain types (Task, Event, Setlist)

### Plane (real-time reference)
- **Key pattern:** Separate live server (Hocuspocus/Yjs) for real-time, REST API for structured data
- **Lesson:** We can add a WebSocket sync service to task-server (using yrs-axum) alongside the existing REST API

### Trilium Notes (sync reference)
- **Key pattern:** Entity change log — every modification creates a tracked change record
- **Sync:** Hub-and-spoke, hash-based integrity verification, last-write-wins with revision history
- **Lesson:** Start with Trilium's pragmatic approach (change tracking + LWW + revisions), evolve toward full CRDTs

### AFFiNE (CRDT reference)
- **Key pattern:** OctoBase Rust data engine with Yjs CRDTs, block-based documents
- **Lesson:** Each .md file section could be an independently mergeable CRDT block

## Local-First Sync Crates

| Crate | Version | What it does |
|---|---|---|
| `datacake` | 0.7.1 | Batteries-included distributed systems framework with eventual consistency |
| `synckit-core` | 0.3.0 | Sync engine specifically for local-first apps |
| `p2panda-sync` | 0.5.2 | Append-only log sync with extensible traits |
| `ensync` | 1.0.2 | Encrypted file sync for untrusted storage |
| `vdsl-sync` | 0.2.0 | Multi-location file sync with pluggable backends |
| `automerge-persistent` | 0.4.0 | Persistence adapters for Automerge (fs, sled, localstorage) |

## WebDAV / CalDAV Crates

| Crate | Version | Notes |
|---|---|---|
| `libdav` | 0.10.3 | Already using — CalDAV + CardDAV |
| `fast-dav-rs` | 0.4.2 | High-performance alternative (HTTP/2, batching) |
| `kitchen-fridge` | 0.4.0 | CalDAV over WebDAV abstraction |
| `minicaldav` | 0.8.0 | Minimal CalDAV client |
| `remotefs-webdav` | 0.2.0 | WebDAV via unified remotefs API |
| `vstorage` | 0.7.0 | Common API for ical/vcard storages |

## Competitive Landscape

| Tool | What it does | How Task differs |
|---|---|---|
| **Tasks.md** | Kanban board backed by .md files | No collaboration, no events, no workflows, no Nextcloud |
| **backlog.md** | Git-based YAML frontmatter tasks | CLI-only, no real-time, no team features |
| **Samply** | Music production collaboration (SaaS) | Hosted, proprietary. Task is self-hosted, file-based, open |
| **Plane** | Project management (open source) | Database-backed, no file portability, no events/workflows |
| **AppFlowy** | Notion alternative (local-first) | Document-focused, not production workflow |
| **AFFiNE** | Knowledge base (local-first CRDT) | Document-focused, not production workflow |
| **Outline** | Collaborative wiki | Server-required, no offline, no events |
| **Trilium** | Personal knowledge base with sync | Single-user focused, no team collaboration |

**Task occupies an unserved niche:** production workflow management with local-first file-based architecture, real-time collaboration, and domain-specific schemas (events, setlists, deliverables). No existing tool combines all of these.

## Open Standards to Support

| Standard | What | Our Use |
|---|---|---|
| CalDAV / VTODO (RFC 5545) | Calendar/task sync | Nextcloud Tasks, Apple Reminders, Thunderbird |
| WebDAV | File access over HTTP | Nextcloud, ownCloud, any WebDAV server |
| DAWproject | Cross-DAW session interchange | Support as deliverable format alongside .rpp |
| Markdown + YAML frontmatter | Document format | Our native format (Obsidian, Hugo, Jekyll compatible) |
| iCalendar (RFC 5545) | Event/recurrence format | RRULE for recurring events/tasks |
| vCard (RFC 6350) | Contact format | Personnel/contact management |
