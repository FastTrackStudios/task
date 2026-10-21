# Vault Spec

The markdown tree and how it is read. The tiering contract above it is
[`../../../docs/spec/storage.md`](../../../docs/spec/storage.md); this spec is
the vault's own read and write path.

Current behaviour these rules exist to replace: `VaultEntityStore::get_by_uuid`
resolves through `find` → `list` → `scan`, so a lookup by id iterates every page
and re-parses every page of that type, discarding all but one. Mutation takes a
lock on the whole vault.

---

## Read path

### Lookup by identity is constant-time

t[vault.index.lookup]
Resolving a page by id, or listing the pages of a type, is served from an index
rather than by iterating the vault. Cost is proportional to the result, not to
the number of pages, and adding pages of unrelated types does not slow either
operation.

---

### A page is parsed once per change

t[vault.index.parse-once]
Parsing a page into its typed model happens once per content change, and the
result is reused until the content changes. The cache is keyed by content, not
by clock, so an unchanged file is never re-parsed and a changed one is never
served stale.

---

### Indexing is incremental

t[vault.index.incremental]
A change to one file re-indexes that file. Nothing about the cost of reacting to
an edit scales with the size of the vault, and no edit triggers a full rescan.
A full rebuild remains available and is never required by ordinary operation.

---

### A parse failure costs one page

t[vault.index.tolerant]
A page that fails to parse is skipped, reported with its path and reason, and
leaves every other page readable. Malformed frontmatter never prevents the vault
loading, and the offending file is still listed as an unparsed page rather than
vanishing.

---

## Write path

### A write locks what it touches

t[vault.write.granular]
Concurrent operations on different pages proceed concurrently. A write takes no
lock wider than the pages it modifies, so one slow write does not stall
unrelated reads.

---

### Writes are atomic per file

t[vault.write.atomic]
A page is written whole or not at all: readers and other tools never observe a
partially written file, including when the process dies mid-write. Frontmatter
round-trips without reordering or reformatting the parts of the document the
write did not touch.

---

## What leaves the node

### A private directory stays on its node

t[vault.private.node-only]
A directory holding the `.fts-private` marker (`files_proto::consts::PRIVATE_MARKER`)
is node-private: nothing under it is versioned (so it is in no history and no
replica), listed by a browse, a facet listing, the vault manifest or a page
walk, or served, hydrated, restored or written by path — by Files or by the
vault lanes. A head recorded before the marker existed is never materialized
over it. The feature that owns the directory writes the marker before anything
else; the lanes only honour it. The contacts feature's CardDAV accounts
(`Records/contacts/.sync/`) are the first such directory.

---

## Bases

### Link functions and link-follow

t[vault.bases.links]
A `.base` filter or formula can ask about a note's links: `file.links` is
the list of its outgoing wikilinks (body and frontmatter, as link values),
`file.hasLink(x)` holds when the note links to `x` (a link value, a name or
a vault path, matched on the note it resolves to, else on its text), and
`x.exists()` holds when a link resolves to a note in the vault (`file.exists()`
always does). A property path through a link value follows it:
`note.song.key` reads `key` from the note `song` links to, a list of links
maps the read over each, and a link that resolves to nothing reads as
`null`. Formulas (`formula.<name>`) evaluate per row, may reference each
other, and a formula that refers to itself evaluates to `null` rather than
failing the view. Each formula is evaluated at most once per row, however
often it is referenced, so a base costs at most formulas × rows formula
evaluations. Links resolve by vault path first, then by basename, so
`[[Song A]]` and `[[Songs/Song A|A]]` name the same note. A basename two
notes share is ambiguous: a bare `[[Name]]` resolves to neither (it reads
as `null` and `exists()` is false), and only a path link names one of them.

### An agent can run a base

t[vault.bases.query]
The MCP tool `query_base` runs a base for an agent: a `.base` file in the
vault by path, or an inline base YAML, one named view or all of them. It
answers each view's rows (filtered, sorted, grouped and capped by `limit`)
with the view's column values and every formula's value per row, evaluated
as `vault.bases.links` describes. The rows, and the notes links resolve to,
are the notes of the one org the call was admitted to, read through that
org's vault. A caller with no standing in an org is refused before anything
is read. It is a read of the notes, so a library bot is offered it.

One call is bounded: an inline base is at most 64 KiB; at most 5,000 notes
and 64 MiB of them are read, in path order; and each view returns at most
`limit` rows (default 50, at most 200) with its `total` and `truncated`.
The answer reports the notes it read (`total`, `read`, `unreadable` paths,
`capped`), so a note that failed to read, or a vault past the cap, is
visible to the caller rather than silently missing.

Checked by: `apps/server/tests/it/mcp_query_base_e2e.rs` (the seeded
`Vault/Gigs/Gigs.base`, run by a library bot of a person in `acme-audio`
only; refused in `vnt-video` on both MCP lanes), unit tests in
`vault_live::bases` (link functions, link-follow, formulas, `view_table`)
and `vault_obsidian::base_query` (markdown rows with body links, `run_base`).
