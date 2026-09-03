//! Wiki pages served through the vault path.
//!
//! A wiki page is edited with the same editor as a vault note — the
//! `VaultSync` file wire, per-file CRDT collab, the link graph, the
//! live `changes` stream — because every wiki root is *also* a vault
//! root on the org's [`vault::Backend`], addressed as
//! [`vault_id`]`(slug)` = `wiki:<slug>`. The wiki `Pages` service keeps
//! working on the same files; what this module adds is the second door.
//!
//! [`WikiVaults::attach`] is the one registration: the sync root, the
//! graph root, the collab inbound listener, the disk watcher, and a
//! bridge that turns vault-sync events on the wiki id into
//! [`WikiEvent`]s — so the wiki home and sidebar, which listen to the
//! wiki stream, stay live when a page is saved from the editor. It runs
//! at boot for every wiki the org holds and again, through
//! [`WikiVaults::created_hook`], for a wiki created while the server
//! runs — no restart.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use vault_proto::VaultEvent;
use wiki_proto::WikiEvent;

/// The vault-id prefix that marks a wiki root.
pub const PREFIX: &str = "wiki:";

/// The vault id a wiki's pages are served under.
#[must_use]
pub fn vault_id(slug: &str) -> String {
    format!("{PREFIX}{slug}")
}

/// The wiki a vault id names, if it names one.
#[must_use]
pub fn wiki_of(vault_id: &str) -> Option<&str> {
    vault_id.strip_prefix(PREFIX).filter(|s| !s.is_empty())
}

/// Everything a wiki root has to be registered with to be editable
/// through the vault path. Cheap to clone; holds the watcher handles
/// so they live as long as the org does.
#[derive(Clone)]
pub struct WikiVaults {
    org: String,
    sync: vault::Backend,
    graph: vault::GraphBackend,
    collab: vault_collab::VaultCollab,
    wiki: wiki_live::WikiBackend,
    watchers: Arc<Mutex<Vec<vault::sync::WatcherHandle>>>,
}

impl WikiVaults {
    #[must_use]
    pub fn new(
        org: &str,
        sync: vault::Backend,
        graph: vault::GraphBackend,
        collab: vault_collab::VaultCollab,
        wiki: wiki_live::WikiBackend,
    ) -> Self {
        Self {
            org: org.to_owned(),
            sync,
            graph,
            collab,
            wiki,
            watchers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Serve `root` as the vault `wiki:<slug>`: sync + graph root,
    /// collab inbound merge, disk watcher, and the event bridge.
    /// Idempotent per slug apart from the watcher, which is attached
    /// once per call — call once per wiki.
    pub async fn attach(&self, slug: &str, root: &Path) {
        let id = vault_id(slug);
        if let Err(e) = self.sync.add_root(id.clone(), root.to_path_buf()) {
            tracing::warn!(org = %self.org, wiki = slug, "wiki vault root not created: {e}");
            return;
        }
        self.graph.add_root(id.clone(), root.to_path_buf());
        // External writes — the wiki `Pages` service, the CLI, an
        // ingest — merge into whichever wiki docs are open.
        self.collab.watch_vault(&id);
        match self.sync.start_watcher(&id).await {
            Ok(handle) => self
                .watchers
                .lock()
                .expect("wiki watchers poisoned")
                .push(handle),
            Err(e) => {
                tracing::warn!(org = %self.org, wiki = slug, "wiki watcher not attached: {e}")
            }
        }
        self.spawn_bridge(slug, &id).await;
        tracing::info!(org = %self.org, wiki = slug, vault_id = %id, "wiki served as a vault root");
    }

    /// Vault-sync events on the wiki id → wiki events, so subscribers
    /// of the wiki stream (home, sidebar, an open page in another
    /// tab) see an editor save. A write through `Pages::write_page`
    /// announces itself already and is seen again here through the
    /// disk watcher; the duplicate is a second refetch, not a second
    /// write.
    async fn spawn_bridge(&self, slug: &str, id: &str) {
        let mut rx = self.sync.channel(id).await.subscribe();
        let wiki = self.wiki.clone();
        let slug = slug.to_owned();
        tokio::spawn(async move {
            loop {
                let event = match rx.recv().await {
                    Ok(VaultEvent::Put { path, .. }) => {
                        if !wiki_live::backend::is_curated_page_path(&path) {
                            continue;
                        }
                        WikiEvent::PageWritten {
                            path,
                            at: chrono::Utc::now(),
                        }
                    }
                    Ok(VaultEvent::Delete { path }) => {
                        if !wiki_live::backend::is_curated_page_path(&path) {
                            continue;
                        }
                        WikiEvent::PageDeleted {
                            path,
                            at: chrono::Utc::now(),
                        }
                    }
                    Ok(VaultEvent::Resync)
                    | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => WikiEvent::Resync,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                };
                wiki.emit(&slug, event);
            }
        });
    }

    /// The `create_wiki` hook: register the new wiki from the
    /// dispatcher's thread by handing the async attach to `handle`.
    #[must_use]
    pub fn created_hook(
        &self,
        handle: tokio::runtime::Handle,
    ) -> wiki_live::backend::WikiCreatedHook {
        let this = self.clone();
        Arc::new(move |slug: &str, root: &Path| {
            let this = this.clone();
            let slug = slug.to_owned();
            let root: PathBuf = root.to_path_buf();
            handle.spawn(async move { this.attach(&slug, &root).await });
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{vault_id, wiki_of};

    #[test]
    fn wiki_vault_ids_round_trip() {
        assert_eq!(vault_id("music-theory"), "wiki:music-theory");
        assert_eq!(wiki_of("wiki:music-theory"), Some("music-theory"));
        assert_eq!(wiki_of("default"), None);
        assert_eq!(wiki_of("wiki:"), None);
    }
}
