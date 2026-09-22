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
//! What the guest router mounts, and nothing else:
//!
//! - `ReviewService` — `scope` from the link; the review, its comments
//!   and commenting, limited to the linked review and file.
//!   `delete_comment` is refused: a link identifies a review, not a
//!   person, so "one's own comment" cannot be established.
//! - `VersionService` — `chain` for the linked file, for the review
//!   page's version switcher. Every other method is refused.
//! - `MediaService` — `rendition_info` for the linked file.
//! - `TreeService`'s stream sibling — `events`, filtered to this
//!   review's comments and its root's checkpoints.
//!
//! Every allowed call checks the link's scope FIRST and only then
//! reaches the backend, inside [`files::lane::caller::on_behalf_of_link`]
//! — the lanes hold nothing for a guest, and that is the one sanctioned
//! way to act for a link whose scope has been checked here.
//!
//! Attribution (AC 1) is constrained at the boundary: every comment is
//! stamped with the link (`via_link`) server-side, and the visitor's
//! chosen display name is suffixed `(guest)` — an anonymous link
//! holder cannot post as an org member.

use std::sync::Arc;

use files::lane::caller::on_behalf_of_link;
use files::service::media::{ByteTicket, Handoff, HandoffItem, HandoffTarget, Region};
use files::service::review::{GuestScope, NewComment};
use files::service::tree::TreeServiceStreamSource;
use files::service::version::{Occupancy, Resolution};
use files::service::{MediaService, ReviewService, VersionService};
use files::{FilesFault, RootId, RootPath};
use files_proto::id::{CommentId, ContentId, ReviewId, SnapshotId, VersionId};
use files_proto::service::review::ReviewEvent;
use files_proto::service::version::VersionEvent;
use files_proto::{
    BrowseEntry, ChainEntry, CheckpointInfo, DivergenceInfo, FilesEvent, GcReport,
    NewReviewComment, RenditionInfo, RenditionKind, Review, ReviewComment, SnapshotInfo,
};
use media_proto::{AttachmentMediaService, MediaChunk, MediaError, MediaGrant, MediaInfo};
use uuid::Uuid;

use crate::share::{ShareStore, StoredLink};

/// The refusal for everything a review link does not reach.
fn denied<T>(method: &str) -> Result<T, FilesFault> {
    Err(FilesFault::denied(
        format!("{method} (not available on a guest review link)"),
        RootPath::root(),
    ))
}

/// Where in the media a comment sits, in the seconds the review store
/// records — the same narrowing the review lane applies to a member's
/// comment. `Page` and `Bytes` are refused rather than flattened to
/// zero: a note on page 4 silently landing at 0:00 is worse than being
/// told it cannot be carried yet.
fn timecode_of(region: &Region) -> Result<f64, FilesFault> {
    #[allow(clippy::cast_precision_loss)]
    let secs = |ms: u64| ms as f64 / 1000.0;
    match *region {
        Region::Time { start_ms, .. } => Ok(secs(start_ms)),
        Region::Rect { at_ms, .. } => Ok(at_ms.map_or(0.0, secs)),
        Region::Whole => Ok(0.0),
        Region::Page { .. } | Region::Bytes { .. } => Err(FilesFault::invalid(
            "a review comment anchors to a moment in the media",
        )),
    }
}

/// What one guest connection is bound to at upgrade.
struct Bound {
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
}

/// The Files lanes a Review guest sees: the review, its comments, its
/// file's chain and renditions, and a filtered live stream — nothing
/// else in the org. One value implements every mounted lane, so the
/// router holds one binding per connection.
#[derive(Clone)]
pub struct GuestLanes {
    bound: Arc<Bound>,
}

impl GuestLanes {
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
        Self {
            bound: Arc::new(Bound {
                files,
                review,
                shares,
                token: link.token.clone(),
                password_at_connect: link.password_sha256.clone(),
                attribution,
            }),
        }
    }

    /// Re-resolve the link — every allowed call passes through here, so
    /// disable/expiry/password changes are retroactive mid-connection.
    /// Also the guest lane's wide-event seam: architect's per-RPC span
    /// is the wide event; these fields mark it as guest traffic (shape
    /// only — never the token).
    fn live_link(&self) -> Result<StoredLink, FilesFault> {
        use architect_telemetry::wide;
        wide::set("share.guest", true);
        let revoked = || FilesFault::invalid("this link has been revoked");
        let Some(link) = self.bound.shares.resolve(&self.bound.token) else {
            wide::set("share.outcome", "revoked");
            return Err(revoked());
        };
        wide::set("share.label", link.label.clone());
        if !self.still_valid(&link) {
            wide::set("share.outcome", "revoked");
            return Err(revoked());
        }
        wide::set("share.outcome", "ok");
        Ok(link)
    }

    fn still_valid(&self, link: &StoredLink) -> bool {
        !link.disabled
            && !link.expired(chrono::Utc::now().timestamp())
            && link.password_sha256 == self.bound.password_at_connect
    }

    fn review(&self) -> &Review {
        &self.bound.review
    }

    fn review_id(&self) -> ReviewId {
        ReviewId::new(self.bound.review.id)
    }

    fn root_id(&self) -> RootId {
        RootId::new(self.bound.review.root_id)
    }

    fn in_scope(&self, root_id: RootId, path: &RootPath) -> bool {
        root_id.get() == self.bound.review.root_id && path.as_str() == self.bound.review.file_path
    }

    /// The linked file, as the lanes address it.
    fn file(&self) -> Result<RootPath, FilesFault> {
        Ok(RootPath::parse(&self.bound.review.file_path)?)
    }

    /// Refuse anything but the linked file.
    fn scoped(&self, method: &str, root_id: RootId, path: &RootPath) -> Result<(), FilesFault> {
        self.live_link()?;
        if self.in_scope(root_id, path) {
            Ok(())
        } else {
            Err(FilesFault::denied(
                format!("{method} (a guest review link reaches only its own file)"),
                path.clone(),
            ))
        }
    }

    /// Refuse anything but the linked review.
    fn scoped_review(&self, method: &str, review: ReviewId) -> Result<StoredLink, FilesFault> {
        let link = self.live_link()?;
        if review == self.review_id() {
            Ok(link)
        } else {
            denied(method)
        }
    }

    /// Whether a live event is this guest's business: this review's
    /// comments and this root's checkpoints, never anything org-wide.
    fn visible(&self, event: &FilesEvent) -> bool {
        match event {
            FilesEvent::Review(ReviewEvent::CommentAdded(c) | ReviewEvent::CommentDeleted(c)) => {
                c.review_id == self.bound.review.id
            }
            FilesEvent::Version(VersionEvent::Checkpointed(info)) => {
                info.root_id == self.bound.review.root_id
            }
            _ => false,
        }
    }
}

// ── The review's own surface ───────────────────────────────────────

impl ReviewService for GuestLanes {
    /// What this link permits — the guest's first call, resolved from
    /// the link itself so the entry page needs nothing in its URL but
    /// the token.
    async fn scope(&self) -> Result<GuestScope, FilesFault> {
        let link = self.live_link()?;
        let caps = link.capabilities();
        Ok(GuestScope {
            review: self.review_id(),
            can_comment: caps.comment,
            can_download: caps.download,
            expires_at: (link.expires_unix > 0)
                .then(|| chrono::DateTime::from_timestamp(link.expires_unix, 0))
                .flatten(),
        })
    }

    async fn review(&self, review: ReviewId) -> Result<Review, FilesFault> {
        self.scoped_review("review", review)?;
        Ok(self.review().clone())
    }

    async fn playback(
        &self,
        review: ReviewId,
        version: VersionId,
    ) -> Result<ByteTicket, FilesFault> {
        self.scoped_review("playback", review)?;
        let files = self.bound.files.clone();
        on_behalf_of_link(async move { ReviewService::playback(&files, review, version).await })
            .await
    }

    async fn comments(&self, review: ReviewId) -> Result<Vec<ReviewComment>, FilesFault> {
        self.scoped_review("comments", review)?;
        let files = self.bound.files.clone();
        on_behalf_of_link(async move { ReviewService::comments(&files, review).await }).await
    }

    async fn comment(&self, comment: NewComment) -> Result<ReviewComment, FilesFault> {
        let link = self.scoped_review("comment", comment.review)?;
        if !link.capabilities().comment {
            return Err(FilesFault::denied(
                "Comment (this link is view-only — commenting is not enabled)",
                self.file()?,
            ));
        }
        let timecode_secs = timecode_of(&comment.region)?;
        // Identity is constrained at the boundary: whatever name the
        // visitor typed, it can't read as an org member's.
        let name = comment.author.trim();
        let author = if name.is_empty() {
            "Guest".to_string()
        } else {
            format!("{name} (guest)")
        };
        // Straight to the backend's attributed entry point rather than
        // through the lane's `comment`: the lane cannot see the link, so
        // it can only stamp "a guest link", and the owner needs to know
        // WHICH link said this.
        let files = self.bound.files.clone();
        let attribution = self.bound.attribution.clone();
        let review_id = comment.review.get();
        let added = NewReviewComment {
            timecode_secs,
            author,
            body: comment.body,
            commit_id: comment.version.commit_prefix(),
            annotation: comment.strokes,
        };
        on_behalf_of_link(async move {
            files
                .add_review_comment_via(review_id, added, attribution)
                .await
        })
        .await
        .map_err(|e| FilesFault::invalid(e.to_string()))
    }

    /// Refused: a link identifies a review, not a person, and two
    /// visitors holding one link are indistinguishable.
    async fn delete_comment(&self, _comment: CommentId) -> Result<ReviewComment, FilesFault> {
        denied("delete_comment")
    }

    async fn for_file(&self, root_id: RootId, path: RootPath) -> Result<Review, FilesFault> {
        // Get, never create: the review already exists (the link was
        // minted on it), and a guest must not mint vault entities.
        self.scoped("for_file", root_id, &path)?;
        Ok(self.review().clone())
    }

    async fn find(&self, root_id: RootId, path: RootPath) -> Result<Option<Review>, FilesFault> {
        self.live_link()?;
        Ok(self.in_scope(root_id, &path).then(|| self.review().clone()))
    }

    /// The guest's "what can I see": exactly the one review this link
    /// scopes to — never the org's list.
    async fn reviews(&self, _root_id: Option<RootId>) -> Result<Vec<Review>, FilesFault> {
        self.live_link()?;
        Ok(vec![self.review().clone()])
    }
}

// ── History: the linked file's chain, nothing more ─────────────────

impl VersionService for GuestLanes {
    async fn chain(&self, root_id: RootId, path: RootPath) -> Result<Vec<ChainEntry>, FilesFault> {
        self.scoped("chain", root_id, &path)?;
        let files = self.bound.files.clone();
        on_behalf_of_link(async move { VersionService::chain(&files, root_id, path).await }).await
    }

    async fn checkpoint(
        &self,
        _root_id: RootId,
        _description: Option<String>,
    ) -> Result<CheckpointInfo, FilesFault> {
        denied("checkpoint")
    }

    async fn snapshots(
        &self,
        _root_id: RootId,
        _limit: Option<u32>,
    ) -> Result<Vec<SnapshotInfo>, FilesFault> {
        denied("snapshots")
    }

    async fn hold(&self, _root_id: RootId, _path: RootPath) -> Result<Occupancy, FilesFault> {
        denied("hold")
    }

    async fn occupancy(
        &self,
        _root_id: RootId,
        _path: RootPath,
    ) -> Result<Vec<Occupancy>, FilesFault> {
        denied("occupancy")
    }

    async fn divergences(&self, _root_id: RootId) -> Result<Vec<DivergenceInfo>, FilesFault> {
        denied("divergences")
    }

    async fn resolve_divergence(
        &self,
        _root_id: RootId,
        _version: VersionId,
        _resolution: Resolution,
    ) -> Result<DivergenceInfo, FilesFault> {
        denied("resolve_divergence")
    }

    async fn restore(
        &self,
        _root_id: RootId,
        _path: RootPath,
        _version: VersionId,
    ) -> Result<ChainEntry, FilesFault> {
        denied("restore")
    }

    async fn keep_snapshot(
        &self,
        _root_id: RootId,
        _snapshot: SnapshotId,
    ) -> Result<CheckpointInfo, FilesFault> {
        denied("keep_snapshot")
    }

    async fn browse_at(
        &self,
        _root_id: RootId,
        _path: RootPath,
        _version: VersionId,
    ) -> Result<Vec<BrowseEntry>, FilesFault> {
        denied("browse_at")
    }

    async fn copy_forward(
        &self,
        _root_id: RootId,
        _version: VersionId,
        _paths: Vec<RootPath>,
    ) -> Result<Vec<RootPath>, FilesFault> {
        denied("copy_forward")
    }

    async fn hint_activity(
        &self,
        _root_id: RootId,
        _paths: Vec<RootPath>,
    ) -> Result<u32, FilesFault> {
        denied("hint_activity")
    }

    async fn collect(
        &self,
        _root_id: RootId,
        _keep_newer_secs: Option<u64>,
    ) -> Result<GcReport, FilesFault> {
        denied("collect")
    }
}

// ── Media: the linked file's renditions, nothing more ──────────────

impl MediaService for GuestLanes {
    async fn read(&self, _root_id: RootId, _path: RootPath) -> Result<ByteTicket, FilesFault> {
        denied("read")
    }

    async fn read_at(
        &self,
        _root_id: RootId,
        _path: RootPath,
        _version: VersionId,
    ) -> Result<ByteTicket, FilesFault> {
        denied("read_at")
    }

    async fn read_content(&self, _content: ContentId) -> Result<ByteTicket, FilesFault> {
        denied("read_content")
    }

    async fn renditions(
        &self,
        _root_id: RootId,
        _path: RootPath,
    ) -> Result<Vec<RenditionInfo>, FilesFault> {
        denied("renditions")
    }

    async fn rendition(
        &self,
        _root_id: RootId,
        _path: RootPath,
        _kind: RenditionKind,
    ) -> Result<ByteTicket, FilesFault> {
        denied("rendition")
    }

    /// The record a player needs to address a rendition on the media
    /// route — at the current content or at a version the switcher
    /// picked. Only the linked file's.
    async fn rendition_info(
        &self,
        root_id: RootId,
        path: RootPath,
        kind: RenditionKind,
        at: Option<VersionId>,
    ) -> Result<RenditionInfo, FilesFault> {
        self.scoped("rendition_info", root_id, &path)?;
        let files = self.bound.files.clone();
        on_behalf_of_link(async move {
            MediaService::rendition_info(&files, root_id, path, kind, at).await
        })
        .await
    }

    async fn handoff(
        &self,
        _name: String,
        _target: HandoffTarget,
        _items: Vec<HandoffItem>,
    ) -> Result<Handoff, FilesFault> {
        denied("handoff")
    }
}

// ── Live: this review's comments and its root's checkpoints ────────

impl TreeServiceStreamSource for GuestLanes {
    /// The org's stream, filtered in process to what this guest may see.
    /// Narrowing to any root but the review's own hears nothing. The link
    /// is re-checked per event, so a revoked link goes quiet mid-stream
    /// exactly as its calls start failing.
    fn events_attach(&self, root_id: Option<RootId>, sink: architect::vox::Tx<FilesEvent>) {
        if self.live_link().is_err() || root_id.is_some_and(|r| r != self.root_id()) {
            return;
        }
        let this = self.clone();
        let mut rx = self.bound.files.subscribe_events();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let live = this
                            .bound
                            .shares
                            .resolve(&this.bound.token)
                            .is_some_and(|link| this.still_valid(&link));
                        if !live {
                            return;
                        }
                        if !this.visible(&event) {
                            continue;
                        }
                        if sink.send(event).await.is_err() {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
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
    inner: crate::media::AttachmentMediaServiceImpl,
    shares: Arc<ShareStore>,
    token: String,
    password_at_connect: Option<String>,
    allowed_prefix: String,
}

impl GuestMediaService {
    pub fn new(
        inner: crate::media::AttachmentMediaServiceImpl,
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

impl AttachmentMediaService for GuestMediaService {
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
