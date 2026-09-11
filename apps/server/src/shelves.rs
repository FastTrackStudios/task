//! [`ShelfRegistry`] — the **one** place a shelf becomes collaborative.
//!
//! Every shelf an org holds — its vault, each of its wikis, each of its
//! asset groups — is registered here and nowhere else, by
//! [`ShelfRegistry::attach`], driven by [`org_proto::Shelf`].
//!
//! # What this replaced, and why the duplication was a defect
//!
//! Before this module the server reached the same machinery twice,
//! spelled differently:
//!
//! ```text
//! the vault:  Backend::single("default", root)
//!             GraphBackend::single("default", root)
//!             vault_collab.watch_vault("default")
//!             vault_sync_state.start_watcher("default")
//!
//! each wiki:  WikiVaults::attach(slug, root)   // the same four calls
//! ```
//!
//! Two spellings of one act, and the cost is not aesthetic. A tier is
//! collaborative if and only if all four calls happened for it, and
//! nothing anywhere checks that they did. Forget `watch_vault` on a new
//! tier and its files are silently not collaborative: no error, no
//! warning, no failing test — an external write simply never reaches an
//! open document, and two people editing one file silently clobber each
//! other. That is the worst class of bug this codebase can produce, and
//! a second registration path is how you get one.
//!
//! So the vault does not have its own path any more. It is one shelf in
//! the same loop as the rest, and the only thing the loop asks about it
//! is [`org_proto::Shelf::vault_id`] and [`org_proto::Shelf::root`].
//!
//! # The one thing that is per-tier, and it is additive
//!
//! A wiki gets a second listener that turns vault-sync events on its id
//! into [`WikiEvent`]s, so the wiki home and sidebar stay live when a
//! page is saved through the editor. That is a *bridge to another
//! feature's event stream*, not part of becoming collaborative, and it
//! is keyed on [`org_proto::Tier`] rather than being a separate call
//! site. A shelf that wants no bridge does not opt out of anything; it
//! simply has no bridge, and it is collaborative all the same.

use std::path::Path;
use std::sync::{Arc, Mutex};

use org_proto::{Shelf, Tier};
use vault_proto::VaultEvent;
use wiki_proto::WikiEvent;

/// The vault-id prefix that marks a wiki root.
///
/// Kept as a constant here because callers outside the registration
/// loop parse it back — see [`wiki_of`]. The composition itself is
/// [`org_proto::Shelf::vault_id`]'s, so there is still one place that
/// decides the spelling.
pub const PREFIX: &str = "wiki:";

/// The vault id a wiki's pages are served under.
#[must_use]
pub fn vault_id(slug: &str) -> String {
    org_proto::WikiShelf::new(slug.to_owned(), std::path::PathBuf::new()).vault_id()
}

/// The wiki a vault id names, if it names one.
#[must_use]
pub fn wiki_of(vault_id: &str) -> Option<&str> {
    vault_id.strip_prefix(PREFIX).filter(|s| !s.is_empty())
}

/// Everything a shelf has to be registered with to be collaborative.
/// Cheap to clone; holds the watcher handles so they live as long as
/// the org does.
#[derive(Clone)]
pub struct ShelfRegistry {
    org: String,
    sync: vault::Backend,
    graph: vault::GraphBackend,
    collab: vault_collab::VaultCollab,
    wiki: Option<wiki_live::WikiBackend>,
    watchers: Arc<Mutex<Vec<vault::sync::WatcherHandle>>>,
}

impl ShelfRegistry {
    #[must_use]
    pub fn new(
        org: &str,
        sync: vault::Backend,
        graph: vault::GraphBackend,
        collab: vault_collab::VaultCollab,
    ) -> Self {
        Self {
            org: org.to_owned(),
            sync,
            graph,
            collab,
            wiki: None,
            watchers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Attach the wiki backend, so wiki shelves also get the event
    /// bridge.
    ///
    /// Optional because the wiki plugin can be compiled out, and a
    /// build without it still has a vault and asset shelves that must
    /// be registered. Without a backend a wiki shelf is registered
    /// exactly like any other and simply announces nothing — which is
    /// the correct behaviour in a build that has no wiki UI to
    /// announce to.
    #[must_use]
    pub fn with_wiki(mut self, wiki: wiki_live::WikiBackend) -> Self {
        self.wiki = Some(wiki);
        self
    }

    /// Register one shelf: sync root, graph root, collab inbound merge,
    /// disk watcher — and, for a wiki, the event bridge.
    ///
    /// Idempotent per vault id apart from the watcher, which is
    /// attached once per call, so call once per shelf.
    ///
    /// The shelf's directory is created if it is missing. A shelf whose
    /// root does not exist yet is the ordinary case for a fresh org's
    /// asset groups, and refusing to register one would make the first
    /// write to it a `NotFound` rather than a file.
    pub async fn attach(&self, shelf: &dyn Shelf) {
        let id = shelf.vault_id();
        let root = shelf.root();
        if let Err(e) = self.sync.add_root(id.clone(), root.to_path_buf()) {
            tracing::warn!(
                org = %self.org,
                shelf = %id,
                "shelf root not created: {e}"
            );
            return;
        }
        self.graph.add_root(id.clone(), root.to_path_buf());
        // External writes — a service, the CLI, an ingest, a person
        // with vim — merge into whichever documents are open.
        self.collab.watch_vault(&id);
        match self.sync.start_watcher(&id).await {
            Ok(handle) => self
                .watchers
                .lock()
                .expect("shelf watchers poisoned")
                .push(handle),
            Err(e) => {
                tracing::warn!(org = %self.org, shelf = %id, "shelf watcher not attached: {e}");
            }
        }
        if shelf.tier() == Tier::Wiki {
            self.spawn_wiki_bridge(shelf.name(), &id).await;
        }
        tracing::info!(
            org = %self.org,
            shelf = %id,
            tier = shelf.tier().as_str(),
            subscribable = shelf.is_subscribable(),
            root = %root.display(),
            "shelf registered for sync, graph and CRDT"
        );
    }

    /// Vault-sync events on a wiki id → wiki events, so subscribers of
    /// the wiki stream (home, sidebar, an open page in another tab) see
    /// an editor save. A write through `Pages::write_page` announces
    /// itself already and is seen again here through the disk watcher;
    /// the duplicate is a second refetch, not a second write.
    async fn spawn_wiki_bridge(&self, slug: &str, id: &str) {
        let Some(wiki) = self.wiki.clone() else {
            return;
        };
        let mut rx = self.sync.channel(id).await.subscribe();
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

    /// The `create_wiki` hook: register a wiki made while the server
    /// runs, from the dispatcher's thread, by handing the async attach
    /// to `handle`.
    #[must_use]
    pub fn created_hook(
        &self,
        handle: tokio::runtime::Handle,
    ) -> wiki_live::backend::WikiCreatedHook {
        let this = self.clone();
        Arc::new(move |slug: &str, root: &Path| {
            let shelf = org_proto::WikiShelf::new(slug.to_owned(), root.to_path_buf());
            let this = this.clone();
            handle.spawn(async move { this.attach(&shelf).await });
        })
    }

    /// The same hook for a **project** declared while the server runs.
    ///
    /// A project's directory is a shelf, and the boot loop only sees
    /// the shelves that exist at boot. Without this, a project created
    /// through the lane — or promoted out of a part, or adopted — would
    /// have an unregistered directory until the next restart: its page
    /// silently not collaborative, its files absent from the link
    /// graph, and nothing failing to say so. That is the exact failure
    /// this module's own docs describe, reached through a door that did
    /// not exist when they were written, and it is why the projects
    /// tier is the first one to need a hook that assets never did.
    ///
    /// Registration is idempotent per vault id apart from the watcher,
    /// so a project whose page is rewritten does not accumulate
    /// registrations — `announce_shelf` fires on the three verbs that
    /// bring a directory into existence as a project (`create`,
    /// `promote_part`, `adopt`) and not on every save.
    #[must_use]
    pub fn project_created_hook(
        &self,
        handle: tokio::runtime::Handle,
    ) -> project::ProjectCreatedHook {
        let this = self.clone();
        Arc::new(move |rel: &str, root: &Path| {
            let shelf = org_proto::ProjectShelf::new(rel.to_owned(), root.to_path_buf());
            let this = this.clone();
            handle.spawn(async move { this.attach(&shelf).await });
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
        assert_eq!(
            wiki_of("assets:songs"),
            None,
            "an asset shelf is not a wiki, and the id says so"
        );
    }
}
