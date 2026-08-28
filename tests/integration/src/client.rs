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
    AccessServiceClient, FederationServiceClient, MediaServiceClient, ReviewServiceClient,
    RootsServiceClient, TreeServiceClient, VersionServiceClient, WriteServiceClient,
};
use files_sync::SyncServiceClient;
use project::ProjectServiceClient;
use task_proto::TaskServiceClient;

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
    /// The server's id, and nothing else. This is the whole of what a
    /// person was given when they registered the device — no host, no
    /// port, no certificate — so it is the whole of what a `Session`
    /// gets to hold.
    server: iroh::EndpointId,
    bearer: Option<Bearer>,
}

impl Session {
    /// Sign in to `server` as the holder of `token`.
    pub async fn open(server: &Server, token: impl Into<String>) -> Self {
        Self {
            device: device_endpoint().await,
            server: server.endpoint.id(),
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
            server: server.endpoint.id(),
            bearer: None,
        }
    }

    async fn establish<C: Signable + vox::FromVoxLane>(&self) -> C {
        let link = iroh_link::connect(&self.device, self.server)
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

    /// The organise lane — tags, favourites, and the activity feed.
    pub async fn organise(&self) -> files::OrganiseServiceClient {
        self.establish().await
    }

    /// The live event stream — `FilesService`'s `#[subscribe]` sibling.
    ///
    /// Its own lane and its own connection, which is the point: what
    /// this chapter asserts is that a client which did *not* make a
    /// change hears about it.
    pub async fn files_stream(&self) -> files::FilesServiceStreamClient {
        self.establish().await
    }

    /// The search lane — extraction and region hits.
    pub async fn search(&self) -> files::SearchServiceClient {
        self.establish().await
    }

    /// The review lane — what a client is shown instead of the tree.
    ///
    /// Reached here as a member, on the org router. A guest reaches the
    /// same backend through `/org/<slug>/share/<token>/vox`, which is
    /// HTTP because a browser opening a link has no other way in.
    pub async fn review(&self) -> ReviewServiceClient {
        self.establish().await
    }

    /// The org's projects — vault pages, not Files roots.
    pub async fn projects(&self) -> ProjectServiceClient {
        self.establish().await
    }

    /// The org's tasks.
    pub async fn tasks(&self) -> TaskServiceClient {
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

/// The endpoint every session in this process dials from.
///
/// One, shared, and bound once. It was one per `Session`, which is
/// tidier to describe and produced a genuinely nasty flake: a `Session`
/// owns its endpoint, so `s.as_alice().await.projects().await` — a
/// temporary — dropped the endpoint at the end of the statement and the
/// client it had just handed back failed its next call with
/// `SendFailed`. Sometimes. Depending on timing.
///
/// Sharing costs nothing here. Sessions are people's devices, and one
/// machine holding several people's logins is the ordinary case; what
/// distinguishes them on the wire is the per-call bearer, which beats
/// anything the connection carries. Servers dial from their *own*
/// endpoints ([`crate::server::Server::dial_replica`]), which is where
/// endpoint identity actually means something.
///
/// "Once" is once per *runtime*, not once per process. An iroh endpoint
/// is a handle onto tasks that live on the runtime that bound it; under
/// `#[tokio::test]` every test is its own runtime, so an endpoint bound
/// by the first test in a binary is a dead handle by the second — every
/// dial from it fails with an internal consistency error. nextest never
/// saw this because it runs one test per process; `cargo test` saw
/// nothing else.
///
/// So the cache is thread-local. A `#[tokio::test]` runtime is
/// current-thread and lives on the test's own thread, which makes
/// "per thread" exactly "per runtime" — and it is also what rules out
/// the race a process-wide slot with a liveness check still had: one
/// test handing out an endpoint in the instant before the test that
/// bound it returns. A multi-thread runtime would bind one per worker
/// that happens to poll here, which costs a socket and nothing else.
async fn device_endpoint() -> iroh::Endpoint {
    thread_local! {
        static DEVICE: std::cell::RefCell<Option<iroh::Endpoint>> = const { std::cell::RefCell::new(None) };
    }
    if let Some(endpoint) = DEVICE.with(|slot| slot.borrow().clone()) {
        return endpoint;
    }
    let endpoint = crate::net::bind(iroh::SecretKey::generate()).await;
    DEVICE.with(|slot| *slot.borrow_mut() = Some(endpoint.clone()));
    endpoint
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
    files::FilesServiceStreamClient,
    files::OrganiseServiceClient,
    files::SearchServiceClient,
    ReviewServiceClient,
    RootsServiceClient,
    TreeServiceClient,
    WriteServiceClient,
    MediaServiceClient,
    VersionServiceClient,
    AccessServiceClient,
    FederationServiceClient,
    SyncServiceClient,
    ProjectServiceClient,
    TaskServiceClient,
);
