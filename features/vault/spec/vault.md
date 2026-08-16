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
