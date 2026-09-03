//! Vault RPC calls — the `VaultSync` round-trips the page makes.
//!
//! Split out of the page component so `vault/mod.rs` is UI, not
//! transport. Every one of these is a plain `async fn` over the org's
//! vault client, on both targets: the native arms were "not wired yet"
//! stubs from before native `establish_for` existed, which made the
//! desktop's vault page a permanent error card.

use vault_proto::PageMeta;

use super::seed_note_bytes;

/// Frontmatter-derived page index for the folder tree. `vault_id` is
/// the org's own vault ([`crate::document_session::VAULT_ID`]) or a
/// wiki's (`wiki:<slug>`) — the same index either way.
pub(crate) async fn fetch_folder_index(
    slug: String,
    vault_id: String,
) -> Result<Vec<PageMeta>, String> {
    let client = crate::vox_clients::vault_client(&slug).await?;
    let idx = client
        .folder_index(vault_id)
        .await
        .map_err(|e| format!("folder_index: {e:?}"))?;
    let mut pages: Vec<PageMeta> = idx
        .pages
        .into_iter()
        .filter(|p| {
            // Notes AND base views — a `.base` is a first-class
            // vault citizen: it appears in
            // the tree, deep-links, and renders its view in place.
            std::path::Path::new(&p.path)
                .extension()
                .is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("base")
                })
        })
        .collect();
    pages.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    Ok(pages)
}

/// Outgoing wikilinks of `path`, via the `VaultGraph` RPC.
pub(crate) async fn fetch_links(
    slug: String,
    vault_id: String,
    path: String,
) -> Result<Vec<vault_proto::GraphLink>, String> {
    let client = crate::vox_clients::vault_graph_client(&slug).await?;
    client
        .links(vault_id, path)
        .await
        .map_err(|e| format!("links: {e:?}"))
}

/// Pages linking to `path`, via the `VaultGraph` RPC.
pub(crate) async fn fetch_backlinks(
    slug: String,
    vault_id: String,
    path: String,
) -> Result<Vec<String>, String> {
    let client = crate::vox_clients::vault_graph_client(&slug).await?;
    client
        .backlinks(vault_id, path)
        .await
        .map_err(|e| format!("backlinks: {e:?}"))
}

/// Re-file a note: set its `folder` to `parent` (None = root)
/// via the server-side frontmatter splice. `prev_sha` empty →
/// unconditional. Returns the freshly committed sha.
pub(super) async fn move_to_folder(
    slug: String,
    vault_id: String,
    path: String,
    parent: Option<String>,
    prev_sha: String,
) -> Result<String, String> {
    let client = crate::vox_clients::vault_client(&slug).await?;
    use vault_proto::IfMatch;
    let if_match = if prev_sha.is_empty() {
        IfMatch::Force
    } else {
        IfMatch::Sha(prev_sha)
    };
    let ack = client
        .set_folder(vault_id, path, parent, if_match)
        .await
        .map_err(|e| format!("set_folder: {e:?}"))?;
    Ok(ack.sha256)
}

/// Create a new note (create-only), seeded with the starter
/// frontmatter scaffold ([`seed_note_bytes`]) so the Properties
/// panel opens with `created`/`tags`/`aliases` already present.
/// Returns its sha.
pub(crate) async fn create_new_file(
    slug: String,
    vault_id: String,
    path: String,
) -> Result<String, String> {
    let client = crate::vox_clients::vault_client(&slug).await?;
    use vault_proto::IfMatch;
    let ack = client
        .put_file(vault_id, path, seed_note_bytes(), IfMatch::CreateOnly)
        .await
        .map_err(|e| format!("put_file: {e:?}"))?;
    Ok(ack.sha256)
}
