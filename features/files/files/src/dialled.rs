//! A client on a vox connection this side dialled, which shuts that
//! connection down when the last copy of it drops.
//!
//! # Why this exists
//!
//! vox keeps a connection (its driver tasks, the link and, over iroh,
//! the QUIC stream under it) until something tells it to shut down or
//! the link ends. Dropping a client only closes that client's *lane*.
//! `initiator_on(link).establish()` drops the connection's handle and
//! returns just the client, so nothing can ever shut that connection
//! down: every dial leaves both ends running for as long as both
//! processes live — the dialler's driver, and the far side's
//! `serve_link` task parked on a stream that never finishes.
//!
//! Measured on the server's device sweep, that was ~0.7 MiB of heap per
//! sweep that never came back (`apps/server/tests/it/memory_budget.rs`).
//! The sync agent redialling its peers every tick had the same leak on
//! the far side, where it costs the *server* an accept task per laptop
//! tick.
//!
//! # The rule
//!
//! A dial over iroh goes through [`Dialled::establish`], not a bare
//! `establish()`. The connection lives exactly as long as some copy of
//! the [`Dialled`] does: clones share it (a sync pass holding a clone
//! while a redial replaces the stored one keeps it until the pass is
//! done), and the last drop shuts it down.
//!
//! Over a pooled QUIC connection (one vox connection per bi-stream, see
//! [`crate::remotes`]) the shutdown finishes that one stream; the QUIC
//! connection stays pooled for the next call. Over a connection of its
//! own the stream's end is the connection's last handle going, so the
//! QUIC connection closes too and the far side's accept loop ends.

use std::sync::Arc;

use architect::iroh_link::IrohLink;
use architect::vox::{ConnectionError, ConnectionHandle, FromVoxLane};

/// A service client, plus the vox connection it rides when this side
/// dialled one. Deref to the client to use it.
pub struct Dialled<C> {
    client: C,
    /// `None` for a client this side did not dial — an in-process lane,
    /// a test's — which has no connection of its own to close.
    hangup: Option<Arc<Hangup>>,
}

/// Shuts the connection down when the last [`Dialled`] sharing it drops.
struct Hangup(ConnectionHandle);

impl Drop for Hangup {
    fn drop(&mut self) {
        // An error means the connection is already gone, which is the
        // outcome we wanted.
        let _ = self.0.shutdown();
    }
}

impl<C: FromVoxLane> Dialled<C> {
    /// Handshake on `link` and open `C`'s lane, keeping the connection.
    ///
    /// A lane that fails to open shuts the connection down too: the
    /// handshake already succeeded, so without that the far side would
    /// keep a session nobody can use.
    ///
    /// # Errors
    ///
    /// The handshake or the lane open failed.
    pub async fn establish(link: IrohLink) -> Result<Self, ConnectionError> {
        let connection = architect::vox::initiator_on(link)
            .establish_connection()
            .await?;
        let hangup = Hangup(connection);
        let client = hangup.0.open_lane::<C>().await?;
        Ok(Self {
            client,
            hangup: Some(Arc::new(hangup)),
        })
    }
}

impl<C> Dialled<C> {
    /// A client this side did not dial, carried in the same slot as one
    /// it did. Dropping it closes nothing beyond what dropping `client`
    /// already would.
    #[must_use]
    pub const fn unowned(client: C) -> Self {
        Self {
            client,
            hangup: None,
        }
    }

    /// Whether dropping the last copy of this shuts a connection down.
    #[must_use]
    pub const fn owns_connection(&self) -> bool {
        self.hangup.is_some()
    }
}

impl<C> From<C> for Dialled<C> {
    fn from(client: C) -> Self {
        Self::unowned(client)
    }
}

impl<C: Clone> Clone for Dialled<C> {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            hangup: self.hangup.clone(),
        }
    }
}

impl<C> std::ops::Deref for Dialled<C> {
    type Target = C;

    fn deref(&self) -> &C {
        &self.client
    }
}

impl<C> std::fmt::Debug for Dialled<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dialled")
            .field("client", &std::any::type_name::<C>())
            .field("owns_connection", &self.owns_connection())
            .finish()
    }
}

/// A far side that counts its live vox sessions, for the tests that
/// prove a dial's session ends. Here rather than in each test because
/// the count is the whole point and has to be taken the same way
/// everywhere: a session is live from its bi-stream's accept until
/// `serve_link` returns, which is when its stream finished.
#[cfg(test)]
pub(crate) mod far_side {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use architect::iroh_link::{self, iroh};

    /// Live sessions on a far side, and its endpoint.
    pub(crate) struct FarSide {
        pub(crate) endpoint: iroh::Endpoint,
        pub(crate) live: Arc<AtomicUsize>,
    }

    impl FarSide {
        /// Bind an endpoint into `book` and serve an empty router on it,
        /// counting sessions. An empty router is enough: what is
        /// measured is the session, not what is called on it.
        pub(crate) async fn serve(book: &crate::AddressBook) -> Self {
            let endpoint = crate::bind_endpoint(iroh::SecretKey::generate(), Some(book.clone()))
                .await
                .expect("bind the far side");
            book.add_endpoint_info(endpoint.addr());
            let live = Arc::new(AtomicUsize::new(0));
            let accepting = endpoint.clone();
            let counter = live.clone();
            tokio::spawn(async move {
                while let Some(incoming) = accepting.accept().await {
                    let counter = counter.clone();
                    tokio::spawn(async move {
                        let Ok(connection) = incoming.await else {
                            return;
                        };
                        while let Ok((send, recv)) = connection.accept_bi().await {
                            let link = iroh_link::IrohLink::new(connection.clone(), send, recv);
                            let counter = counter.clone();
                            counter.fetch_add(1, Ordering::SeqCst);
                            tokio::spawn(async move {
                                let router = architect::LayerRouter::new();
                                iroh_link::serve_link(
                                    link,
                                    architect::layer::handler_acceptor(router),
                                )
                                .await;
                                counter.fetch_sub(1, Ordering::SeqCst);
                            });
                        }
                    });
                }
            });
            Self { endpoint, live }
        }

        /// Sessions open now.
        pub(crate) fn live(&self) -> usize {
            self.live.load(Ordering::SeqCst)
        }

        /// Wait up to `within` for the live count to reach `n`.
        pub(crate) async fn settles_at(&self, n: usize, within: Duration) -> bool {
            let deadline = tokio::time::Instant::now() + within;
            while tokio::time::Instant::now() < deadline {
                if self.live() == n {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            self.live() == n
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use architect::iroh_link::{self, iroh};

    use super::Dialled;
    use super::far_side::FarSide;

    async fn dialler(book: &crate::AddressBook) -> iroh::Endpoint {
        let endpoint = crate::bind_endpoint(iroh::SecretKey::generate(), Some(book.clone()))
            .await
            .expect("bind the dialler");
        book.add_endpoint_info(endpoint.addr());
        endpoint
    }

    /// The bug this type exists for, pinned so a vox upgrade that fixes
    /// it upstream shows up here: a bare `establish()` leaves the far
    /// side's session running after its client is dropped.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_bare_establish_leaves_the_far_session_running() {
        let book = crate::AddressBook::new();
        let far = FarSide::serve(&book).await;
        let near = dialler(&book).await;

        let link = iroh_link::connect(&near, far.endpoint.id())
            .await
            .expect("dial");
        let client: crate::RootsServiceClient = architect::vox::initiator_on(link)
            .establish()
            .await
            .expect("establish");
        assert!(far.settles_at(1, Duration::from_secs(5)).await);
        drop(client);
        assert!(
            !far.settles_at(0, Duration::from_secs(1)).await,
            "a dropped client closes only its lane; the session should still be up"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_the_last_copy_ends_the_far_session() {
        let book = crate::AddressBook::new();
        let far = FarSide::serve(&book).await;
        let near = dialler(&book).await;

        let link = iroh_link::connect(&near, far.endpoint.id())
            .await
            .expect("dial");
        let dialled = Dialled::<crate::RootsServiceClient>::establish(link)
            .await
            .expect("establish");
        assert!(dialled.owns_connection());
        assert!(far.settles_at(1, Duration::from_secs(5)).await);

        // A clone (a sync pass mid-flight) keeps it up...
        let held = dialled.clone();
        drop(dialled);
        assert!(
            !far.settles_at(0, Duration::from_millis(500)).await,
            "a live clone must keep the connection"
        );
        // ...and the last drop takes it down.
        drop(held);
        assert!(
            far.settles_at(0, Duration::from_secs(5)).await,
            "the far side's session should end with the last copy"
        );
    }

    #[test]
    fn an_unowned_client_owns_no_connection() {
        let dialled: Dialled<u8> = 7.into();
        assert!(!dialled.owns_connection());
        assert_eq!(*dialled, 7);
    }
}
