//! Who a peer is, and what a peer may do — `files.peering.*`.
//!
//! A peer is a machine that holds part of an org: another server, a
//! laptop, a backup shelf. It is not a person and it does not have an
//! account, so none of the identity machinery built for people applies to
//! it. What it has is the identity the transport already proved.
//!
//! # The credential is the connection
//!
//! An iroh connection is mutually authenticated by construction, so the
//! endpoint id on the far end was demonstrated during the handshake
//! rather than claimed afterwards. There is nothing to issue, store,
//! expire, rotate or leak: the credential is the endpoint you are already
//! dialling by. Admission is therefore a *list*, not a secret — which
//! endpoints this store will serve — and that list is
//! [`FilesBackend::admit_host`].
//!
//! # Why this is not in `apps/server`
//!
//! It was, and that put it out of reach of the callers who need it most.
//! A device serving its own content to another device is the whole point
//! of `files.topology.multi-server`'s "where two peers can reach each
//! other, bytes move directly over iroh/QUIC" — and a device cannot
//! depend on the server binary to find out who it may talk to.
//!
//! So the peer's authorization model lives with the peering feature. A
//! server composes it into its org gate alongside sessions and roles; a
//! laptop uses it alone, because for a laptop it is the whole answer:
//! there are no accounts on a laptop, only endpoints it admits.

use std::sync::Arc;

use architect_permissions::{
    Action, Decision, IdentityResolver, PermissionEngine, Principal, Resource,
};

use crate::backend::FilesBackend;

/// The prefix a peer presents instead of a session token.
///
/// `host:<endpoint-id>`. Not a secret and not meant to be — the endpoint
/// id is public, and what proves the caller holds it is the handshake
/// that happened before this string was assembled. The transport is the
/// credential; this only tells the gate what the transport already
/// verified.
///
/// Which is why nothing but the transport may set it. A serve loop
/// derives it from the connection's remote id, and a client cannot forge
/// it because a *per-call* `authorization` beats the connection's — so a
/// caller claiming `host:` in metadata is resolved against the admitted
/// set under an id it does not hold, and fails.
pub const HOST_BEARER_PREFIX: &str = "host:";

/// The replica lane's own coarse resource, and not `files/**`.
///
/// A role scoped to `files/**` can read every files lane there is. A peer
/// needs exactly this one, so it needs a resource that means exactly this
/// one — otherwise "may replicate" and "may read the whole org" are the
/// same permission, and admitting a machine becomes the widest grant on
/// the server.
pub const REPLICA_RESOURCE: &str = "files/replica";

/// What the replica lane's methods require.
///
/// One definition, used twice: a server folds it into its org gate beside
/// forty other tables, and a device installs it as the only table it has.
/// Two copies would be two things to keep in step, and the failure mode of
/// them drifting is a method a server serves and a device refuses — or
/// worse, the reverse.
///
/// `chunks` and `chunk_ranges` are `download` rather than `read` because
/// they are how a whole library leaves, a batch or a window at a time, and
/// "who pulled a copy of everything" is the question an audit log exists
/// to answer.
pub const REPLICA_PERMITS: architect_permissions::ServicePermits =
    architect_permissions::ServicePermits {
        service: "files-replica",
        methods: &[
            architect_permissions::MethodPermit::new("roots", Action::READ, REPLICA_RESOURCE),
            architect_permissions::MethodPermit::new("heads", Action::READ, REPLICA_RESOURCE),
            architect_permissions::MethodPermit::new("object", Action::READ, REPLICA_RESOURCE),
            architect_permissions::MethodPermit::new("manifest", Action::READ, REPLICA_RESOURCE),
            architect_permissions::MethodPermit::new("chunks", Action::DOWNLOAD, REPLICA_RESOURCE)
                .audited(),
            architect_permissions::MethodPermit::new(
                "chunk_ranges",
                Action::DOWNLOAD,
                REPLICA_RESOURCE,
            )
            .audited(),
        ],
    };

/// Resolve an admitted peer to a principal of its own.
///
/// Peers are not people and must not borrow a person's identity to be
/// one. Before this existed, replicating an org meant presenting some
/// member's session token — so "which machines may hold this org" was
/// answered by "who happens to have a login", and admitting a server
/// meant issuing it a human's credential.
///
/// A non-`host:` bearer falls through untouched, so this composes in
/// front of a session resolver rather than replacing it. On a device
/// there is nothing to fall through *to*, and
/// [`AnonymousFallback`] is what says so.
pub struct HostResolver<R> {
    inner: R,
    /// The admitted set to check against. Per-store because admission is:
    /// one machine can hold several orgs and be a stranger to the next.
    files: FilesBackend,
    /// Names the store in log lines — an org slug on a server, a device
    /// name on a laptop.
    whose: String,
}

impl<R> HostResolver<R> {
    pub fn new(inner: R, files: FilesBackend, whose: impl Into<String>) -> Self {
        Self {
            inner,
            files,
            whose: whose.into(),
        }
    }
}

impl<R: IdentityResolver> IdentityResolver for HostResolver<R> {
    fn resolve<'a>(
        &'a self,
        bearer_token: Option<&'a str>,
    ) -> architect_permissions::BoxIdentityFuture<'a> {
        Box::pin(async move {
            let Some(endpoint) = bearer_token.and_then(|t| t.strip_prefix(HOST_BEARER_PREFIX))
            else {
                return self.inner.resolve(bearer_token).await;
            };

            let host = files_domain::HostId(endpoint.to_string());
            if self.files.admits(&host).is_none() {
                // A verified endpoint this store never admitted. One warn
                // line: a machine dialling content it does not hold is
                // either a misconfiguration or a probe, and both are
                // worth seeing.
                tracing::warn!(
                    whose = self.whose,
                    host = endpoint,
                    "peering: an unadmitted peer presented itself — refusing"
                );
                return Principal::Anonymous;
            }
            // Its own variant, and each alternative was wrong for its own
            // reason: `User` picks up a default member role, `Service`
            // rides the role engine's in-process bypass, and a `Guest`'s
            // credential is a link somebody minted rather than the peer's
            // own proved identity. `Principal::Host` carries no rights at
            // all — [`HostEngine`] is the only thing that grants it any.
            Principal::Host {
                endpoint: endpoint.to_string(),
            }
        })
    }
}

/// An identity resolver for a store with no accounts.
///
/// A device has no auth database and never will: the only callers it can
/// recognise are endpoints it admits. Composed under [`HostResolver`],
/// this is what "and nobody else" means.
pub struct AnonymousFallback;

impl IdentityResolver for AnonymousFallback {
    fn resolve<'a>(
        &'a self,
        _bearer_token: Option<&'a str>,
    ) -> architect_permissions::BoxIdentityFuture<'a> {
        Box::pin(async { Principal::Anonymous })
    }
}

/// What an admitted peer may do: read the replica lane, and nothing.
///
/// The narrowness is the point. A peer needs the commit graph and the
/// chunks under it to converge an org's structure; it does not need to
/// write, to grant, to browse the vault, or to read anyone's mail. On
/// `files/**` — the resource every other files lane shares — "read" would
/// have meant all of those.
#[derive(Debug, Default)]
pub struct HostEngine;

impl PermissionEngine for HostEngine {
    fn check(&self, who: &Principal, what: &Resource, action: &Action) -> Decision {
        if matches!(who, Principal::Host { .. })
            && what.as_str() == REPLICA_RESOURCE
            && matches!(action.as_str(), "read" | "download")
        {
            return Decision::Allow;
        }
        // Deny rather than abstain: a composite is first-allow-wins, so
        // this only ever narrows what another engine already refused.
        Decision::deny(format!(
            "{} may not {} {}",
            who.describe(),
            action.as_str(),
            what.as_str()
        ))
    }

    fn survey(&self, who: &Principal, _prefix: &Resource) -> Vec<(Resource, Vec<Action>)> {
        if !matches!(who, Principal::Host { .. }) {
            return Vec::new();
        }
        vec![(
            Resource::new(REPLICA_RESOURCE),
            vec![Action::READ.into(), Action::DOWNLOAD.into()],
        )]
    }
}

/// Serve a router on an iroh endpoint, one guarded handler per
/// connection.
///
/// The router has to be built **per connection**, because the identity is
/// a property of the connection rather than of the server: on an iroh
/// connection there is no upgrade to read a token from, and what there is
/// instead is `connection.remote_id()` — proved by the handshake, not
/// presented afterwards. `make` receives it as a `host:` bearer and
/// returns the handler to serve that peer.
///
/// This exists once rather than twice because the invariant is easy to
/// lose. `iroh_link::serve_router` serves ONE router to every caller,
/// which is right for a handler that treats all callers alike and wrong
/// for anything gated — and using it is how the org router came to be
/// served with no gate at all: nothing at the call site says an identity
/// is missing, because an ungated router answers every call perfectly
/// well.
pub async fn serve_over_iroh<H, F>(endpoint: &architect::iroh_link::iroh::Endpoint, make: F)
where
    F: Fn(Option<String>) -> H + Send + Sync + Clone + 'static,
    H: architect::vox::Handler<architect::vox::DriverReplySink> + Clone + Send + Sync + 'static,
{
    while let Some(incoming) = endpoint.accept().await {
        let make = make.clone();
        tokio::spawn(async move {
            let connection = match incoming.await {
                Ok(connection) => connection,
                Err(err) => {
                    tracing::warn!(%err, "peering: incoming iroh connection failed");
                    return;
                }
            };
            // Proved by the handshake, not asserted by the caller.
            let bearer = format!("{HOST_BEARER_PREFIX}{}", connection.remote_id());
            let handler = make(Some(bearer));

            loop {
                match connection.accept_bi().await {
                    Ok((send, recv)) => {
                        let link =
                            architect::iroh_link::IrohLink::new(connection.clone(), send, recv);
                        let acceptor = architect::layer::handler_acceptor(handler.clone());
                        tokio::spawn(architect::iroh_link::serve_link(link, acceptor));
                    }
                    Err(err) => {
                        tracing::debug!(%err, "peering: iroh connection closed");
                        return;
                    }
                }
            }
        });
    }
}

/// The permission gate a **device** serves its peers through.
///
/// Everything a server's gate has and nothing it does not: no sessions,
/// no roles, no org membership — a device has no accounts to check
/// against. Which endpoints it admits is the entire question, and
/// [`HostEngine`] is the entire answer.
///
/// `enforce` is not optional here as it is on a server. Observe-only
/// exists so a deployment can turn on a permit table and watch before
/// refusing; a device's gate has one rule that has always been true, so
/// there is nothing to observe first.
#[must_use]
pub fn device_gate(
    files: &FilesBackend,
    whose: &str,
) -> architect::permissions_gate::PermissionsGate {
    use architect::permissions_gate::{PermissionsGate, UnlistedPolicy};

    let identity: Arc<dyn IdentityResolver + Send + Sync> =
        Arc::new(HostResolver::new(AnonymousFallback, files.clone(), whose));
    PermissionsGate::new(Arc::new(HostEngine), identity)
        // A method with no permit row is refused. On a server that is
        // `Allow`, because an org router mounts forty services and a
        // missing row should not take one down; a device mounts exactly
        // one lane, so anything unlisted is something it did not mean to
        // serve.
        .unlisted(UnlistedPolicy::Deny)
        .observe_only(false)
}
