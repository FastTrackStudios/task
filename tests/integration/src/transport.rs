//! The wire between servers.
//!
//! `files` states what it needs of another server — browse a granted
//! subtree, mint a ticket in it, pull a bounded chunk — and this
//! supplies it over iroh. The backend never learns that a connection
//! exists, let alone how one is made, which is the same seam placement
//! uses for storage boundaries.
//!
//! Nothing else in this example knows about QUIC either. That is the
//! point of the port: swap this file for a WebSocket and the scenario
//! is unchanged.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use architect::iroh_link;
use files::path::RootPath;
use files::service::federation::{ByteRange, EndpointId};

/// The `RemoteFiles` port, over iroh.
///
/// `files` states what it needs of another server; this supplies it. The
/// backend never learns that a connection exists, let alone how one is
/// made — the same seam placement uses for storage boundaries.
///
/// Dialling per call is deliberate for a demo: it makes each browse
/// obviously a real network round trip rather than a warm cache. A
/// server pools connections.
#[derive(Debug)]
pub struct IrohRemotes {
    pub endpoint: iroh::Endpoint,
    /// Endpoint id → address. A demo stands in for the address lookup a
    /// deployment gets from iroh's discovery; the id is still the only
    /// thing a user ever handles.
    pub known: Arc<Mutex<HashMap<String, iroh::EndpointAddr>>>,
}

impl IrohRemotes {
    /// Dial an origin and open its federation lane.
    ///
    /// A connection per call, which a demo can afford and a deployment
    /// would not — pooling belongs here, and its absence is why a
    /// relayed read costs a handshake per chunk below.
    async fn dial(
        &self,
        origin: &EndpointId,
    ) -> Result<files::FederationServiceClient, files::FilesFault> {
        let addr = self
            .known
            .lock()
            .expect("known peers")
            .get(&origin.0)
            .cloned()
            .ok_or_else(|| files::FilesFault::Unavailable {
                path: RootPath::root(),
            })?;
        let link = iroh_link::connect(&self.endpoint, addr)
            .await
            .map_err(|e| files::FilesFault::Io(format!("dial {origin}: {e}")))?;
        // `establish` does the handshake and opens the service lane in
        // one step — the same call the CLI makes over a WebSocket, with
        // only the link underneath it different.
        vox_core::initiator_on(link)
            .establish()
            .await
            .map_err(|e| files::FilesFault::Io(format!("establish: {e}")))
    }
}

/// Unwrap a vox error into the fault the origin actually raised.
pub fn fault(e: vox::VoxError<files::FilesFault>) -> files::FilesFault {
    match e {
        vox::VoxError::User(fault) => *fault,
        other => files::FilesFault::Io(other.to_string()),
    }
}

#[async_trait::async_trait]
impl files::lane::federation::RemoteFiles for IrohRemotes {
    async fn read_offered(
        &self,
        origin: &EndpointId,
        secret: &str,
        path: &RootPath,
    ) -> Result<files::service::media::ByteTicket, files::FilesFault> {
        self.dial(origin)
            .await?
            .read_offered(secret.to_string(), path.clone())
            .await
            .map_err(fault)
    }

    async fn fetch_offered(
        &self,
        origin: &EndpointId,
        secret: &str,
        token: &str,
        range: ByteRange,
    ) -> Result<Vec<u8>, files::FilesFault> {
        self.dial(origin)
            .await?
            .fetch_offered(secret.to_string(), token.to_string(), range)
            .await
            .map_err(fault)
    }

    async fn browse_offered(
        &self,
        origin: &EndpointId,
        secret: &str,
        path: &RootPath,
    ) -> Result<Vec<files::model::BrowseEntry>, files::FilesFault> {
        self.dial(origin)
            .await?
            .browse_offered(secret.to_string(), path.clone())
            .await
            .map_err(fault)
    }
}
