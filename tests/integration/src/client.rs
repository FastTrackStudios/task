//! One concept: a person talking to a server.
//!
//! Everything a chapter asserts should go through here rather than
//! through a backend value, because "the backend computes the right
//! answer" and "a client can get that answer" are different claims and
//! only the second one is the product. The gap between them is where
//! this repo's real failures have lived: a lane implemented and never
//! mounted, a permit table that did not cover a method, two services
//! whose names collided on the router.
//!
//! # How identity travels
//!
//! As `authorization: Bearer <token>` in per-call vox metadata, pushed
//! by a client middleware. That is transport-agnostic, which matters
//! here: the browser's channel is a WebSocket subprotocol read at
//! upgrade, and there is no upgrade on an iroh connection. Per-call
//! metadata is the one channel both ends of this suite can use.
//!
//! The gate reads per-call metadata first and falls back to whatever
//! the connection presented at establish. Only the first of those
//! exists over iroh, so it is the only one exercised here.

use architect::iroh_link;
use files::{
    AccessServiceClient, FederationServiceClient, MediaServiceClient, RootsServiceClient,
    TreeServiceClient, VersionServiceClient, WriteServiceClient,
};
use files_sync::SyncServiceClient;

use crate::server::Server;

/// Signs every call with a session token.
///
/// A missing token would be an anonymous call rather than a failed one,
/// which is why this holds a `String` and not an `Option`: a client
/// built without an identity should be built by not installing this,
/// so that "signed out" is visible at the call site.
#[derive(Debug, Clone)]
pub struct Bearer(pub String);

impl vox::ClientMiddleware for Bearer {
    fn pre<'a, 'call>(
        &'a self,
        _context: &'a vox::ClientContext<'a>,
        request: &'a mut vox::ClientRequest<'call, 'a>,
    ) -> vox::BoxMiddlewareFuture<'a> {
        let value = format!("Bearer {}", self.0);
        Box::pin(async move {
            request.push_string_metadata("authorization", value);
        })
    }
}

/// One person's connection to one server.
///
/// A connection per service lane, which a deployment would pool and a
/// test can afford. Each accessor dials, so every call in a chapter is
/// a real round trip through the real router with the real gate in
/// front of it.
pub struct Session {
    /// The endpoint this person dials *from*. A person is not a server,
    /// so this is their device's endpoint rather than an org's.
    device: iroh::Endpoint,
    addr: iroh::EndpointAddr,
    bearer: Option<Bearer>,
}

impl Session {
    /// Sign in to `server` as the holder of `token`.
    pub async fn open(server: &Server, token: impl Into<String>) -> Self {
        Self {
            device: device_endpoint().await,
            addr: server.endpoint.addr(),
            bearer: Some(Bearer(token.into())),
        }
    }

    /// Connect to `server` presenting nothing.
    ///
    /// Not a convenience — the negative half of every access claim. A
    /// suite that can only make signed-in calls cannot tell a rule that
    /// works from a rule that is never consulted.
    pub async fn anonymous(server: &Server) -> Self {
        Self {
            device: device_endpoint().await,
            addr: server.endpoint.addr(),
            bearer: None,
        }
    }

    async fn establish<C: Signable + vox::FromVoxLane>(&self) -> C {
        let link = iroh_link::connect(&self.device, self.addr.clone())
            .await
            .expect("dial the server");
        let client: C = vox_core::initiator_on(link)
            .establish()
            .await
            .expect("establish a service lane");
        match &self.bearer {
            Some(bearer) => client.signed(bearer.clone()),
            None => client,
        }
    }

    pub async fn roots(&self) -> RootsServiceClient {
        self.establish().await
    }

    pub async fn tree(&self) -> TreeServiceClient {
        self.establish().await
    }

    pub async fn write(&self) -> WriteServiceClient {
        self.establish().await
    }

    pub async fn media(&self) -> MediaServiceClient {
        self.establish().await
    }

    pub async fn version(&self) -> VersionServiceClient {
        self.establish().await
    }

    pub async fn access(&self) -> AccessServiceClient {
        self.establish().await
    }

    pub async fn federation(&self) -> FederationServiceClient {
        self.establish().await
    }

    /// The replica lane: the commit graph and the chunks under it.
    ///
    /// A peer host calls this, not a person — but it is signed with a
    /// person's token, because a host has no credential of its own.
    /// See `peering.rs` for what that costs.
    pub async fn replica(&self) -> SyncServiceClient {
        self.establish().await
    }
}

/// A device's own endpoint.
///
/// Fresh per session, because a device is not a server and has no
/// persisted identity to reuse — and because two sessions sharing one
/// endpoint would make "who dialled" ambiguous the moment a test cares.
async fn device_endpoint() -> iroh::Endpoint {
    iroh_link::bind_endpoint(iroh::SecretKey::generate())
        .await
        .expect("bind a device endpoint")
}

/// The one thing every generated client has in common that this file
/// needs: a way to add the bearer middleware after `establish`.
///
/// `establish` is generic over the client type and the middleware call
/// is inherent on each generated struct, so without this each accessor
/// above would be the same six lines written out.
pub trait Signable: Sized {
    fn signed(self, bearer: Bearer) -> Self;
}

macro_rules! signable {
    ($($client:ty),* $(,)?) => {
        $(impl Signable for $client {
            fn signed(self, bearer: Bearer) -> Self {
                self.with_middleware(bearer)
            }
        })*
    };
}

signable!(
    RootsServiceClient,
    TreeServiceClient,
    WriteServiceClient,
    MediaServiceClient,
    VersionServiceClient,
    AccessServiceClient,
    FederationServiceClient,
    SyncServiceClient,
);
