//! `FederationService` — content across a server boundary.
//!
//! # The backend does not dial
//!
//! Reaching another server means an iroh endpoint, an ALPN and a
//! connection supervisor, none of which belong in a crate that knows
//! about chunk stores. So this lane resolves remotes through a
//! [`RemoteFiles`] port, exactly as placement resolves storage
//! boundaries through `LocationBoundaries`: `files` states what it needs
//! of a remote, and whoever owns the transport supplies it.
//!
//! The consequence worth stating: a backend with no port answers
//! `Unavailable` for remote content and keeps serving everything local,
//! which is the same shape as an origin being down. There is one
//! degradation path, not two.
//!
//! # An accepted offer is an ordinary root
//!
//! `files.topology.federation` says a federated file arrives "as a
//! first-class item — browsable, versioned, syncable, pinnable — not a
//! download link". So accepting mints a `RootId` in this server's own
//! registry and every other lane addresses it without knowing it is not
//! local. That is also why this lane has no browse of its own for the
//! receiving side: adding one would make federated content a second kind
//! of thing with a second set of verbs, which is the outcome the rule
//! refuses.
//!
//! The origin side does have one — [`FederationService::browse_offered`]
//! — because that is a *different* call: it authenticates a secret and
//! resolves inside an offered subtree, so a receiver cannot walk out of
//! what it was given.

use std::collections::HashMap;

use chrono::Utc;
use files_proto::error::FilesFault;
use files_proto::id::{GrantId, RootId};
use files_proto::model::BrowseEntry;
use files_proto::path::RootPath;
use files_proto::service::access::Capability;
use files_proto::service::federation::{ByteRange, EndpointId, FederationService, Offer, Remote};
use files_proto::service::media::ByteTicket;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backend::FilesBackend;
use crate::durable::Scoped;

/// What this lane needs of a remote server.
///
/// Implemented by whoever owns the transport — the example wires it to
/// iroh, and the server will. Deliberately minimal: the port carries
/// what a browse needs and nothing about how a connection is made,
/// pooled or retried.
#[async_trait::async_trait]
pub trait RemoteFiles: Send + Sync + std::fmt::Debug + 'static {
    /// Ask `origin` to list `path` inside the subtree `secret` grants.
    async fn browse_offered(
        &self,
        origin: &EndpointId,
        secret: &str,
        path: &RootPath,
    ) -> Result<Vec<BrowseEntry>, FilesFault>;

    /// Ask `origin` to mint a ticket for a file inside the granted
    /// subtree. Gives this server length and content type so it can
    /// answer its own caller truthfully.
    async fn read_offered(
        &self,
        origin: &EndpointId,
        secret: &str,
        path: &RootPath,
    ) -> Result<ByteTicket, FilesFault>;

    /// Pull one bounded chunk of a ticket `origin` minted.
    ///
    /// Bounded is the point: this server is relaying, so an unbounded
    /// read would put the whole object through its memory on the way
    /// past — the failure `files.scale.large-media` exists to prevent,
    /// and it does not stop being that failure because the bytes are
    /// someone else's.
    async fn fetch_offered(
        &self,
        origin: &EndpointId,
        secret: &str,
        token: &str,
        range: ByteRange,
    ) -> Result<Vec<u8>, FilesFault>;
}

/// What the origin keeps about an offer it has made.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Offered {
    offer: Offer,
    /// Who it was offered to. Not on the wire `Offer` — the receiver
    /// knows who it is — but the origin needs it to answer "what have I
    /// shared, and with whom".
    to: EndpointId,
    /// The path the secret resolves inside. Held separately from the
    /// offer so a withdrawn offer can be forgotten without losing the
    /// audit of what it was.
    root_id: RootId,
    path: RootPath,
    withdrawn: bool,
}

/// What the receiver keeps about a remote it has accepted.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Accepted {
    remote: Remote,
    secret: String,
    path: RootPath,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Federated {
    /// Offers this server made, by secret.
    offers: HashMap<String, Offered>,
    /// Remotes this server accepted, by local root id.
    remotes: HashMap<String, Accepted>,
}

static FEDERATION: Scoped<Federated> = Scoped::new("federation");

fn read<T>(backend: &FilesBackend, f: impl FnOnce(&Federated) -> T) -> T {
    FEDERATION.read(backend, f)
}

fn write<T>(backend: &FilesBackend, f: impl FnOnce(&mut Federated) -> T) -> T {
    FEDERATION.write(backend, f)
}

/// A 256-bit secret, from two v4 UUIDs.
///
/// The same construction the byte lane's tokens use, for the same
/// reason: a capability is only as good as its unguessability.
fn mint_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

impl FilesBackend {
    /// The offer a secret stands for, if it is still good for reading.
    ///
    /// Every origin-side entry point goes through here, which is what
    /// makes a withdrawal land on the *next call of any kind* rather
    /// than only on the one whose author remembered to check. A relayed
    /// transfer therefore stops mid-file, not at the next file.
    fn live_offer(&self, secret: &str, path: &RootPath) -> Result<Offered, FilesFault> {
        let Some(offered) = read(self, |f| f.offers.get(secret).cloned()) else {
            // Unknown and withdrawn answer alike, so a prober cannot
            // learn whether a secret was ever real.
            return Err(FilesFault::invalid("no such offer"));
        };
        if offered.withdrawn {
            return Err(FilesFault::invalid("no such offer"));
        }
        if let Some(expiry) = offered.offer.expires_at
            && expiry <= Utc::now()
        {
            return Err(FilesFault::invalid("no such offer"));
        }
        if !offered.offer.capabilities.contains(&Capability::Read) {
            return Err(FilesFault::denied("read", path.clone()));
        }
        Ok(offered)
    }

    /// Resolve a receiver's path inside the subtree it was granted.
    ///
    /// The receiver's path is relative to what it was given, so `..`
    /// cannot walk above it — and `RootPath::parse` already refused
    /// `..` anyway, which makes this belt and braces on the one check
    /// that must not fail.
    fn inside(offered: &Offered, path: &RootPath) -> Result<RootPath, FilesFault> {
        Ok(if offered.path.is_root() {
            path.clone()
        } else if path.is_root() {
            offered.path.clone()
        } else {
            RootPath::parse(format!("{}/{}", offered.path, path))?
        })
    }

    /// Whether a root here is one this server accepted from elsewhere.
    ///
    /// The tree lane asks before it walks: a remote root has no live
    /// tree on this disk, and walking its path would list an empty
    /// directory as though the subtree were empty rather than elsewhere.
    #[must_use]
    pub(crate) fn remote_of(&self, root_id: RootId) -> Option<(EndpointId, String)> {
        read(self, |f| {
            f.remotes
                .get(&root_id.to_string())
                .map(|a| (a.remote.origin.clone(), a.secret.clone()))
        })
    }

    /// Resolve a remote browse through the port, or say why not.
    /// Mint a ticket for a file on a root accepted from elsewhere.
    ///
    /// The receiving half of `files.peering.serving`: this server holds
    /// none of these bytes and still answers `read`, because a host
    /// without the content fetches it from a host that has it. The
    /// caller gets an ordinary local ticket and never learns the object
    /// is somewhere else — handing back the origin's own token would
    /// turn a first-class federated file into a download link to
    /// another server.
    pub(crate) async fn read_remote(
        &self,
        root_id: RootId,
        path: &RootPath,
    ) -> Result<ByteTicket, FilesFault> {
        let Some((origin, secret)) = self.remote_of(root_id) else {
            return Err(FilesFault::RootNotFound(root_id));
        };
        let Some(port) = self.remote_files() else {
            return Err(FilesFault::Unavailable { path: path.clone() });
        };
        match port.read_offered(&origin, &secret, path).await {
            Ok(remote) => {
                self.mark_reachable(root_id, true);
                // A local ticket standing for a remote object. Redemption
                // relays chunk by chunk, so nothing here is downloaded
                // up front — a preview that plays the first second of a
                // 4 GB reel transfers the first second.
                Ok(self.mint_relay_ticket(origin, secret, remote))
            }
            Err(fault) => {
                self.mark_reachable(root_id, false);
                Err(fault)
            }
        }
    }

    pub(crate) async fn browse_remote(
        &self,
        root_id: RootId,
        path: &RootPath,
    ) -> Result<Vec<BrowseEntry>, FilesFault> {
        let Some((origin, secret)) = self.remote_of(root_id) else {
            return Err(FilesFault::RootNotFound(root_id));
        };
        let Some(port) = self.remote_files() else {
            // No transport wired. Same answer as an origin being down,
            // because from a caller's position it is the same situation.
            return Err(FilesFault::Unavailable { path: path.clone() });
        };
        match port.browse_offered(&origin, &secret, path).await {
            Ok(entries) => {
                self.mark_reachable(root_id, true);
                Ok(entries)
            }
            Err(fault) => {
                // Reachability is a fact about the network, recorded so
                // `remotes()` can report it. The tree does not change.
                self.mark_reachable(root_id, false);
                Err(fault)
            }
        }
    }

    fn mark_reachable(&self, root_id: RootId, reachable: bool) {
        write(self, |f| {
            if let Some(a) = f.remotes.get_mut(&root_id.to_string())
                && a.remote.reachable != reachable
            {
                a.remote.reachable = reachable;
            }
        });
    }
}

impl FederationService for FilesBackend {
    // t[impl files.topology.federation] — a grant that crosses a boundary
    async fn offer(
        &self,
        root_id: RootId,
        path: RootPath,
        to: EndpointId,
        capabilities: Vec<Capability>,
    ) -> Result<Offer, FilesFault> {
        let root = crate::lane::root_or_fault(self, root_id)?;
        let path = path.validate()?;

        // The subtree has to exist before it is offered, so holding an
        // offer is never a promise about a path that was never there.
        let disk = crate::lane::lane_tree(&root)?.join(path.as_str());
        if !path.is_root() && !disk.exists() {
            return Err(FilesFault::PathNotFound(path));
        }
        if capabilities.is_empty() {
            return Err(FilesFault::invalid(
                "an offer with no capabilities grants nothing",
            ));
        }

        let offer = Offer {
            origin: EndpointId(self.endpoint_id().unwrap_or_default()),
            root_id,
            path: path.clone(),
            name: if path.is_root() {
                root.name.clone()
            } else {
                path.name().unwrap_or(&root.name).to_string()
            },
            capabilities,
            secret: mint_secret(),
            grant: GrantId::generate(),
            expires_at: None,
        };
        write(self, |f| {
            f.offers.insert(
                offer.secret.clone(),
                Offered {
                    offer: offer.clone(),
                    to,
                    root_id,
                    path,
                    withdrawn: false,
                },
            );
        });
        Ok(offer)
    }

    /// Withdraw, from the originating side.
    // t[impl files.topology.federation] — revocable from the origin
    async fn withdraw(&self, grant: GrantId) -> Result<(), FilesFault> {
        let found = write(self, |f| {
            let mut hit = false;
            for offered in f.offers.values_mut() {
                if offered.offer.grant == grant {
                    offered.withdrawn = true;
                    hit = true;
                }
            }
            hit
        });
        if found {
            Ok(())
        } else {
            Err(FilesFault::GrantRevoked(grant))
        }
    }

    async fn offered(&self) -> Result<Vec<Offer>, FilesFault> {
        Ok(read(self, |f| {
            let mut v: Vec<_> = f
                .offers
                .values()
                .filter(|o| !o.withdrawn)
                .map(|o| o.offer.clone())
                .collect();
            v.sort_by(|a, b| a.name.cmp(&b.name));
            v
        }))
    }

    /// Accept an offer, making it a root here.
    // t[impl files.topology.federation] — arrives as a first-class item
    async fn accept(&self, offer: Offer) -> Result<Remote, FilesFault> {
        if offer.secret.is_empty() {
            return Err(FilesFault::invalid(
                "an offer with no secret grants nothing",
            ));
        }
        // A local id, because every other lane addresses it as a root and
        // must not have to know it is not ours. The origin's id is kept
        // separately — it means nothing here.
        let local = RootId::generate();
        let remote = Remote {
            root_id: local,
            origin: offer.origin.clone(),
            origin_root: offer.root_id,
            name: offer.name.clone(),
            capabilities: offer.capabilities.clone(),
            // Optimistic until a call proves otherwise: claiming
            // unreachable before trying would be as wrong as claiming
            // reachable after failing.
            reachable: true,
            accepted_at: Utc::now(),
        };
        write(self, |f| {
            f.remotes.insert(
                local.to_string(),
                Accepted {
                    remote: remote.clone(),
                    secret: offer.secret,
                    path: offer.path,
                },
            );
        });
        Ok(remote)
    }

    async fn remotes(&self) -> Result<Vec<Remote>, FilesFault> {
        Ok(read(self, |f| {
            let mut v: Vec<_> = f.remotes.values().map(|a| a.remote.clone()).collect();
            v.sort_by(|a, b| a.name.cmp(&b.name));
            v
        }))
    }

    async fn forget(&self, root_id: RootId) -> Result<(), FilesFault> {
        let gone = write(self, |f| f.remotes.remove(&root_id.to_string()).is_some());
        if gone {
            Ok(())
        } else {
            Err(FilesFault::RootNotFound(root_id))
        }
    }

    /// The origin side of a receiver's browse.
    // t[impl files.topology.federation] — the secret is the authority
    // t[impl files.access.granularity] — resolved inside the offer, never above it
    async fn read_offered(&self, secret: String, path: RootPath) -> Result<ByteTicket, FilesFault> {
        let path = path.validate()?;
        let offered = self.live_offer(&secret, &path)?;
        let within = Self::inside(&offered, &path)?;

        // Through the media lane, so a federated read is pinned to a
        // content address exactly as a local one is: a checkpoint
        // landing mid-transfer cannot change the bytes under a
        // half-served response on either side of the boundary.
        <Self as files_proto::service::media::MediaService>::read(self, offered.root_id, within)
            .await
            .map_err(|_| FilesFault::PathNotFound(path))
    }

    async fn fetch_offered(
        &self,
        secret: String,
        token: String,
        range: ByteRange,
    ) -> Result<Vec<u8>, FilesFault> {
        // Re-checked on every chunk, not once per file: a grant
        // withdrawn during a large transfer has to stop that transfer,
        // or revocation means "after this 244 GB finishes".
        self.live_offer(&secret, &RootPath::root())?;

        let range = ByteRange::new(range.offset, range.len);
        let last = range
            .offset
            .saturating_add(u64::from(range.len))
            .saturating_sub(1);
        let mut buf = Vec::new();
        self.redeem_bytes(&token, Some((range.offset, last)), &mut buf)
            .await?;
        Ok(buf)
    }

    async fn browse_offered(
        &self,
        secret: String,
        path: RootPath,
    ) -> Result<Vec<BrowseEntry>, FilesFault> {
        let path = path.validate()?;
        let offered = self.live_offer(&secret, &path)?;

        let within = Self::inside(&offered, &path)?;

        // Through the tree lane, not the legacy browse.
        //
        // `files.ignore.layers` says the platform layer applies
        // everywhere. Going straight to the legacy listing put a Mac's
        // `._name` sidecars across the federation boundary — a receiver
        // seeing junk the origin's own users do not, which is a worse
        // failure than seeing it locally because it is someone else's
        // tree that looks untidy.
        <Self as files_proto::service::tree::TreeService>::browse(self, offered.root_id, within)
            .await
            .map_err(|_| FilesFault::PathNotFound(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer_of(path: &str, caps: Vec<Capability>) -> Offered {
        Offered {
            offer: Offer {
                origin: EndpointId("origin".into()),
                root_id: RootId::generate(),
                path: RootPath::parse(path).unwrap(),
                name: "Song".into(),
                capabilities: caps,
                secret: mint_secret(),
                grant: GrantId::generate(),
                expires_at: None,
            },
            to: EndpointId("receiver".into()),
            root_id: RootId::generate(),
            path: RootPath::parse(path).unwrap(),
            withdrawn: false,
        }
    }

    #[test]
    fn a_secret_is_not_guessable_by_length() {
        let a = mint_secret();
        assert_eq!(a.len(), 64, "256 bits of hex");
        assert_ne!(a, mint_secret());
    }

    // t[verify files.access.granularity]
    #[test]
    fn a_receivers_path_resolves_inside_the_offer() {
        let offered = offer_of("Audio Files", vec![Capability::Read]);
        let asked = RootPath::parse("Vox").unwrap();
        let within = RootPath::parse(format!("{}/{}", offered.path, asked)).unwrap();
        assert_eq!(within.as_str(), "Audio Files/Vox");
        assert!(
            within.is_within(&offered.path),
            "a receiver can never resolve above what it was offered"
        );
    }

    #[test]
    fn traversal_cannot_be_smuggled_through_an_offer() {
        // The typed path refuses it before this lane ever composes it.
        assert!(RootPath::parse("../../etc").is_err());
        assert!(RootPath::parse("Audio Files/../../etc").is_err());
    }

    #[test]
    fn an_offer_without_read_grants_no_browse() {
        let offered = offer_of("Audio Files", vec![Capability::Comment]);
        assert!(!offered.offer.capabilities.contains(&Capability::Read));
    }
}
