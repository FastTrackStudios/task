// The C ABI of `files-fileprovider` — what the macOS File Provider
// extension links against. See src/lib.rs for what each call means and
// why the split falls where it does.
//
// Error protocol, throughout: a function returning a string answers
// NULL on failure; a function returning int answers -1. Either way the
// reason is available from fts_fp_last_error() until the next call on
// the same thread. Every string this library returns is owned by the
// caller and freed with fts_fp_free().

#ifndef FTS_FILEPROVIDER_H
#define FTS_FILEPROVIDER_H

#ifdef __cplusplus
extern "C" {
#endif

/// Why the last call on this thread failed, or NULL if it did not.
char *fts_fp_last_error(void);

/// Free a string this library returned.
void fts_fp_free(char *s);

/// Facts about one absolute path in a live tree, as JSON:
///   {"size": <bytes>, "dehydrated": <bool>, "executable": <bool>}
/// `size` is the size of the *content*, which for a dehydrated file is
/// what its pointer stub records rather than what stat reports.
char *fts_fp_facts(const char *path);

/// The roots the running agent holds, as JSON:
///   [{"id": "<uuid>", "name": "...", "path": "<abs>"}]
char *fts_fp_roots(void);

/// Bring one path's content resident. Blocks until it is there.
int fts_fp_hydrate(const char *root_id, const char *rel_path);

/// Release one path's bytes, leaving it listed at its real size.
int fts_fp_evict(const char *root_id, const char *rel_path);

#ifdef __cplusplus
}
#endif

#endif /* FTS_FILEPROVIDER_H */
