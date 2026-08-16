//! The share guest lane (issue #272): `/org/{slug}/share/{token}/vox`
//! puts an anonymous visitor in the REAL app's RPC surface, scoped to
//! exactly one Review — same wire types, same clients, so the review
//! player and comment thread work unchanged over a guest connection.
//!
//! The scoping is structural, not advisory: the lane mounts wrapper
//! services that delegate ONLY the review's own calls to the backend
//! and refuse everything else. A guest holds no session and needs
//! none — the token (plus password/expiry) is the whole grant, checked
//! at upgrade AND re-resolved on every call, so revoking, disabling,
//! expiring, or re-passwording a link cuts off already-connected
//! guests at their next request, exactly like the HTTP routes.
//!
//! Attribution (AC 1) is constrained at the boundary: every comment is
//! stamped with the link (`via_link`) server-side, and the visitor's
//! chosen display name is suffixed `(guest)` — an anonymous link
//! holder cannot post as an org member.

use std::sync::Arc;

use files::{FilesError, FilesService};
use files_proto::{
    BrowseEntry, ChainEntry, CheckpointInfo, DivergenceChoice, DivergenceInfo, FileRootInfo,
    FilesEvent, GcReport, NamedVersion, NewReviewComment, ProjectVersion, RenditionInfo,
    RenditionKind, RestartMode, Review, ReviewComment, RootFlavor, SnapshotInfo, VersionRef,
};
use media_proto::{MediaChunk, MediaError, MediaGrant, MediaInfo, MediaService};
use uuid::Uuid;

use crate::share::{ShareStore, StoredLink};

fn denied<T>() -> Result<T, FilesError> {
    Err(FilesError::BadRequest(
        "not available on a guest review link".into(),
    ))
}

/// Aborts the guest event forwarder when the last clone of the lane's
/// services drops (the socket closed).
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The Files surface a Review guest sees: the review, its comments,
/// its file's chain and renditions — nothing else in the org.
#[derive(Clone)]
pub struct GuestFilesService {
    files: files::FilesBackend,
    review: Review,
    /// Live-revocation seam: the link is re-resolved from here on every
    /// call, so owner-side changes are retroactive mid-connection.
    shares: Arc<ShareStore>,
    token: String,
    /// The password hash the connection presented at upgrade — if the
    /// owner changes the password, existing connections stop matching
    /// and die on their next call.
    password_at_connect: Option<String>,
    /// Stamped onto every comment (AC 1) — "label (token-prefix)".
    attribution: String,
    /// The guest's own event hub: a filtered mirror of the org hub
    /// (this review's comments + this root's checkpoints), so the
    /// review page's live subscription works without leaking org-wide
    /// events to an anonymous visitor.
    hub: architect::PubSub<FilesEvent>,
    _forwarder: Arc<AbortOnDrop>,
}

impl GuestFilesService {
    pub fn new(
        files: files::FilesBackend,
        review: Review,
        shares: Arc<ShareStore>,
        link: &StoredLink,
    ) -> Self {
        let attribution = format!(
            "{} ({})",
            link.label,
            &link.token[..8.min(link.token.len())]
        );
        // The filtered event mirror: subscribe to the org's stream
        // in-process and republish only what this guest may see.
        //
        // The subscription MUST ride a vox link (`LocalServer`, the
        // notifier's pattern): a bare paired `vox::channel()` Tx has no
        // sink until a transport binds it, so `PubSub::attach`ing one
        // directly never delivers — `try_send` reports Full forever
        // and the event sits in the hub mailbox.
        let hub = architect::PubSub::sliding(64);
        let guest_hub = hub.clone();
        let (review_id, root_id) = (review.id, review.root_id);
        let stream_backend = files.clone();
        let forwarder = tokio::spawn(async move {
            let scope = architect::Scope::new();
            let local = architect::LocalServer::serve(
                architect::LayerRouter::new()
                    .merge(files_proto::files_service_stream_layer(stream_backend)),
                scope.clone(),
            );
            let Ok(client) = local
                .establish::<files_proto::FilesServiceStreamClient>()
                .await
            else {
                tracing::warn!("share guest: event mirror could not establish");
                return;
            };
            let (tx, mut rx) = vox::channel::<FilesEvent>();
            let call = tokio::spawn(async move { client.events(tx).await });
            while let Ok(Some(msg)) = rx.recv().await {
                // `SelfRef` has no owned extraction; `map` lends the
                // value, so cloning out is sound — events own their
                // data.
                let mut owned: Option<FilesEvent> = None;
                let _ = msg.map(|ev| owned = Some(ev.clone()));
                let Some(event) = owned else { continue };
                let visible = match &event {
                    FilesEvent::ReviewCommentAdded(c) | FilesEvent::ReviewCommentDeleted(c) => {
                        c.review_id == review_id
                    }
                    FilesEvent::Checkpointed(info) => info.root_id == root_id,
                    _ => false,
                };
                if visible {
                    guest_hub.publish(event);
                }
            }
            call.abort();
            scope.close().await;
        });
        Self {
            files,
            review,
            shares,
            token: link.token.clone(),
            password_at_connect: link.password_sha256.clone(),
            attribution,
            hub,
            _forwarder: Arc::new(AbortOnDrop(forwarder)),
        }
    }

    /// Re-resolve the link — every allowed call passes through here, so
    /// disable/expiry/password changes are retroactive mid-connection.
    /// Also the guest lane's wide-event seam: architect's per-RPC span
    /// is the wide event; these fields mark it as guest traffic (shape
    /// only — never the token).
    fn live_link(&self) -> Result<StoredLink, FilesError> {
        use architect_telemetry::wide;
        wide::set("share.guest", true);
        let Some(link) = self.shares.resolve(&self.token) else {
            wide::set("share.outcome", "revoked");
            return Err(FilesError::BadRequest("this link has been revoked".into()));
        };
        wide::set("share.label", link.label.clone());
        if link.disabled
            || link.expired(chrono::Utc::now().timestamp())
            || link.password_sha256 != self.password_at_connect
        {
            wide::set("share.outcome", "revoked");
            return Err(FilesError::BadRequest("this link has been revoked".into()));
        }
        wide::set("share.outcome", "ok");
        Ok(link)
    }

    fn in_scope(&self, root_id: Uuid, path: &str) -> bool {
        root_id == self.review.root_id && path == self.review.file_path
    }
}

impl FilesService for GuestFilesService {
    // ── the review's own surface ──────────────────────────────────

    async fn find_review(
        &self,
        root_id: Uuid,
        file_path: String,
    ) -> Result<Option<Review>, FilesError> {
        self.live_link()?;
        Ok(self
            .in_scope(root_id, &file_path)
            .then(|| self.review.clone()))
    }

    async fn review_for_file(
        &self,
        root_id: Uuid,
        file_path: String,
    ) -> Result<Review, FilesError> {
        // Get, never create: the review already exists (the link was
        // minted on it), and a guest must not mint vault entities.
        self.live_link()?;
        if self.in_scope(root_id, &file_path) {
            Ok(self.review.clone())
        } else {
            denied()
        }
    }

    async fn review_comments(&self, review_id: Uuid) -> Result<Vec<ReviewComment>, FilesError> {
        self.live_link()?;
        if review_id != self.review.id {
            return denied();
        }
        self.files.review_comments(review_id).await
    }

    async fn add_review_comment(
        &self,
        review_id: Uuid,
        comment: NewReviewComment,
    ) -> Result<ReviewComment, FilesError> {
        let link = self.live_link()?;
        if review_id != self.review.id {
            return denied();
        }
        if !link.capabilities().comment {
            return Err(FilesError::BadRequest(
                "this link is view-only — commenting is not enabled".into(),
            ));
        }
        // Identity is constrained at the boundary: whatever name the
        // visitor typed, it can't read as an org member's.
        let name = comment.author.trim();
        let author = if name.is_empty() {
            "Guest".to_string()
        } else {
            format!("{name} (guest)")
        };
        let comment = NewReviewComment { author, ..comment };
        self.files
            .add_review_comment_via(review_id, comment, self.attribution.clone())
            .await
    }

    async fn chain(&self, root_id: Uuid, path: String) -> Result<Vec<ChainEntry>, FilesError> {
        self.live_link()?;
        if !self.in_scope(root_id, &path) {
            return denied();
        }
        self.files.chain(root_id, path).await
    }

    async fn rendition(
        &self,
        root_id: Uuid,
        path: String,
        kind: RenditionKind,
    ) -> Result<RenditionInfo, FilesError> {
        self.live_link()?;
        if !self.in_scope(root_id, &path) {
            return denied();
        }
        self.files.rendition(root_id, path, kind).await
    }

    async fn rendition_at(
        &self,
        root_id: Uuid,
        path: String,
        commit_id: String,
        kind: RenditionKind,
    ) -> Result<RenditionInfo, FilesError> {
        self.live_link()?;
        if !self.in_scope(root_id, &path) {
            return denied();
        }
        self.files
            .rendition_at(root_id, path, commit_id, kind)
            .await
    }

    // ── everything else: refused ──────────────────────────────────

    /// Even root-scoped: version LABELS of the root's other files are
    /// the org's curation vocabulary, not the guest's business — the
    /// review switcher simply renders without stars on the guest lane.
    async fn list_named_versions(
        &self,
        _root_id: Option<Uuid>,
    ) -> Result<Vec<NamedVersion>, FilesError> {
        denied()
    }

    async fn create_root(
        &self,
        _path: String,
        _name: String,
        _flavor: RootFlavor,
    ) -> Result<FileRootInfo, FilesError> {
        denied()
    }
    async fn list_roots(&self) -> Result<Vec<FileRootInfo>, FilesError> {
        denied()
    }
    async fn get_root(&self, _id: Uuid) -> Result<FileRootInfo, FilesError> {
        denied()
    }
    async fn browse(
        &self,
        _root_id: Uuid,
        _subpath: String,
    ) -> Result<Vec<BrowseEntry>, FilesError> {
        denied()
    }
    async fn drive_browse(&self, _path: String) -> Result<Vec<BrowseEntry>, FilesError> {
        denied()
    }

    async fn tree_browse(&self, _path: String) -> Result<files_proto::TreeNode, FilesError> {
        // The org tree spans the whole vault/wiki — nothing a
        // review-scoped guest may see.
        denied()
    }
    async fn checkpoint_now(
        &self,
        _root_id: Uuid,
        _message: Option<String>,
    ) -> Result<CheckpointInfo, FilesError> {
        denied()
    }
    async fn hint_activity(&self, _root_id: Uuid, _paths: Vec<String>) -> Result<u32, FilesError> {
        denied()
    }
    async fn snapshots(&self, _root_id: Uuid) -> Result<Vec<SnapshotInfo>, FilesError> {
        denied()
    }
    async fn ignore_set(&self, _root_id: Uuid) -> Result<Vec<String>, FilesError> {
        denied()
    }
    async fn set_ignore_set(
        &self,
        _root_id: Uuid,
        _patterns: Vec<String>,
    ) -> Result<Vec<String>, FilesError> {
        denied()
    }
    async fn name_version(
        &self,
        _root_id: Uuid,
        _commit_id: String,
        _name: String,
    ) -> Result<NamedVersion, FilesError> {
        denied()
    }
    async fn resolve_named_version(&self, _id: Uuid) -> Result<VersionRef, FilesError> {
        denied()
    }
    async fn unname_version(&self, _id: Uuid) -> Result<(), FilesError> {
        denied()
    }
    async fn start_project_version(
        &self,
        _root_id: Uuid,
        _label: Option<String>,
    ) -> Result<ProjectVersion, FilesError> {
        denied()
    }
    async fn list_project_versions(
        &self,
        _root_id: Uuid,
    ) -> Result<Vec<ProjectVersion>, FilesError> {
        denied()
    }
    async fn gc_root(
        &self,
        _root_id: Uuid,
        _keep_secs: Option<u64>,
    ) -> Result<GcReport, FilesError> {
        denied()
    }
    async fn dehydrate(&self, _root_id: Uuid, _path: String) -> Result<BrowseEntry, FilesError> {
        denied()
    }
    async fn hydrate(&self, _root_id: Uuid, _path: String) -> Result<BrowseEntry, FilesError> {
        denied()
    }
    async fn hydration_policy(&self, _root_id: Uuid) -> Result<Vec<String>, FilesError> {
        denied()
    }
    async fn set_hydration_policy(
        &self,
        _root_id: Uuid,
        _patterns: Vec<String>,
    ) -> Result<Vec<String>, FilesError> {
        denied()
    }
    async fn apply_hydration_policy(
        &self,
        _root_id: Uuid,
    ) -> Result<files_proto::HydrationReport, FilesError> {
        denied()
    }
    async fn restart_project_version(
        &self,
        _root_id: Uuid,
        _mode: RestartMode,
        _label: Option<String>,
    ) -> Result<ProjectVersion, FilesError> {
        denied()
    }
    async fn browse_at(
        &self,
        _root_id: Uuid,
        _commit_id: String,
        _subpath: String,
    ) -> Result<Vec<BrowseEntry>, FilesError> {
        denied()
    }
    async fn copy_forward(
        &self,
        _root_id: Uuid,
        _commit_id: String,
        _paths: Vec<String>,
    ) -> Result<Vec<String>, FilesError> {
        denied()
    }
    async fn divergences(&self, _root_id: Uuid) -> Result<Vec<DivergenceInfo>, FilesError> {
        denied()
    }
    async fn resolve_divergence(
        &self,
        _root_id: Uuid,
        _path: String,
        _choice: DivergenceChoice,
    ) -> Result<CheckpointInfo, FilesError> {
        denied()
    }
    /// The guest's "what can I see" call: exactly the one review this
    /// link scopes to — the entry page resolves its (root, file) from
    /// here rather than carrying them in the URL.
    async fn list_reviews(&self, _root_id: Option<Uuid>) -> Result<Vec<Review>, FilesError> {
        self.live_link()?;
        Ok(vec![self.review.clone()])
    }
    async fn delete_review_comment(&self, _id: Uuid) -> Result<(), FilesError> {
        denied()
    }
}

impl files_proto::service::legacy::FilesServiceStreamSource for GuestFilesService {
    fn events_hub(&self) -> &architect::PubSub<FilesEvent> {
        &self.hub
    }
}

/// The media surface a guest sees: exactly one grant prefix — the
/// review root's renditions — so the `<video>` URLs the player builds
/// work, and nothing else on the media route does.
///
/// The grant is root-wide by the media token's own granularity (the
/// rendition route's path is `files/renditions/{root}/{kind}/{id}` and
/// grants are prefix-scoped). Renditions of OTHER files in the root
/// are reachable only by their 64-hex CAS content ids, which the guest
/// lane never reveals — the RPC scope is the real wall; the token is
/// transport auth. FUTURE: per-file grant subjects would close the
/// gap for leaked ids.
#[derive(Clone)]
pub struct GuestMediaService {
    inner: crate::media::MediaServiceImpl,
    shares: Arc<ShareStore>,
    token: String,
    password_at_connect: Option<String>,
    allowed_prefix: String,
}

impl GuestMediaService {
    pub fn new(
        inner: crate::media::MediaServiceImpl,
        shares: Arc<ShareStore>,
        link: &StoredLink,
        root_id: Uuid,
    ) -> Self {
        Self {
            inner,
            shares,
            token: link.token.clone(),
            password_at_connect: link.password_sha256.clone(),
            allowed_prefix: format!("files/renditions/{root_id}"),
        }
    }

    fn live(&self) -> Result<(), MediaError> {
        let ok = self.shares.resolve(&self.token).is_some_and(|link| {
            !link.disabled
                && !link.expired(chrono::Utc::now().timestamp())
                && link.password_sha256 == self.password_at_connect
        });
        if ok {
            Ok(())
        } else {
            Err(MediaError::Internal("this link has been revoked".into()))
        }
    }
}

impl MediaService for GuestMediaService {
    async fn stat(&self, _content_hash: String) -> Result<MediaInfo, MediaError> {
        Err(MediaError::Internal(
            "not available on a guest review link".into(),
        ))
    }

    async fn read(
        &self,
        _content_hash: String,
        _start: u64,
        _len: u64,
        _tx: vox::Tx<MediaChunk>,
    ) -> Result<(), MediaError> {
        Err(MediaError::Internal(
            "not available on a guest review link".into(),
        ))
    }

    async fn media_grant(&self, prefix: String) -> Result<MediaGrant, MediaError> {
        self.live()?;
        if prefix != self.allowed_prefix {
            return Err(MediaError::Internal(format!(
                "a guest review link may only mint grants for {}",
                self.allowed_prefix
            )));
        }
        self.inner.media_grant(prefix).await
    }
}
