//! What a person does *to* a page rather than in it: rename or move it,
//! and delete it. One place, so the note header, the wiki page and the
//! vault tree say the same thing when it works and when it does not.

/// Move `from` to `to` in `vault_id` (a rename, a move to another folder,
/// or both), after writing `buffer` to `from` so an edit made in the last
/// second — not yet flushed by autosave or the live session — goes with
/// it. Returns the pages whose links were updated to follow.
pub async fn move_page(
    slug: String,
    vault_id: String,
    from: String,
    to: String,
    buffer: Option<Vec<u8>>,
) -> Result<Vec<String>, String> {
    let client = crate::vox_clients::vault_client(&slug).await?;
    #[cfg(target_arch = "wasm32")]
    {
        use vault_proto::IfMatch;
        let guard = match buffer {
            Some(bytes) => IfMatch::Sha(
                client
                    .put_file(vault_id.clone(), from.clone(), bytes, IfMatch::Force)
                    .await
                    .map_err(|e| sentence(&e, &from))?
                    .sha256,
            ),
            None => IfMatch::Force,
        };
        client
            .move_file(vault_id, from, to.clone(), guard)
            .await
            .map(|ack| ack.relinked)
            .map_err(|e| sentence(&e, &to))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (client, vault_id, from, to, buffer);
        Err("native client not wired yet".to_owned())
    }
}

/// Delete `path`, but only if it is still the version the person was
/// looking at (`sha`); a page someone changed meanwhile is not deleted
/// out from under them.
pub async fn delete_page(
    slug: String,
    vault_id: String,
    path: String,
    sha: Option<String>,
) -> Result<(), String> {
    let client = crate::vox_clients::vault_client(&slug).await?;
    #[cfg(target_arch = "wasm32")]
    {
        use vault_proto::IfMatch;
        let guard = sha.map_or(IfMatch::Force, IfMatch::Sha);
        client
            .delete_file(vault_id, path.clone(), guard)
            .await
            .map_err(|e| sentence(&e, &path))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (client, vault_id, path, sha);
        Err("native client not wired yet".to_owned())
    }
}

/// A vault error as something to tell the person who asked.
#[cfg(target_arch = "wasm32")]
fn sentence(e: &vox::VoxError<vault_proto::VaultSyncError>, path: &str) -> String {
    use vault_proto::VaultSyncError as E;
    match e {
        vox::VoxError::User(e) => match &**e {
            E::Conflict { .. } => {
                format!("{path} changed or already exists — reload and try again.")
            }
            E::NotFound => format!("{path} isn't there any more."),
            E::BadPath => format!("“{path}” isn't a usable name."),
            E::Refused(reason) => reason.clone(),
            other => other.to_string(),
        },
        other => format!("{other:?}"),
    }
}

/// "Updated links in 3 pages." — or nothing, when none needed it.
#[must_use]
pub fn relinked_note(relinked: &[String]) -> Option<String> {
    match relinked.len() {
        0 => None,
        1 => Some(format!("Updated the link in {}.", relinked[0])),
        n => Some(format!("Updated links in {n} pages.")),
    }
}
