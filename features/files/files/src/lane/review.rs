//! `ReviewService` — the guest lane.
//!
//! Every other lane's caller is a member of the org and the backend
//! carries their identity. Here the caller is anonymous and **the link
//! is the authorisation**, scoped to one review and nothing else. That
//! single difference decides most of what follows.
//!
//! ## The machinery already exists; this lane is the guest's view of it
//!
//! Reviews, comments, annotations and the get-or-create are v1's, in
//! [`crate::backend`] and [`crate::versions`], and they are correct: one
//! review per `(root, file)`, following renames so a rename does not fork
//! the conversation, and a comment stamped with the version it was made
//! against so a new version landing does not silently re-point it. None
//! of that is re-implemented here. This module translates the v2 shapes
//! ([`ReviewId`], [`VersionId`], [`Region`]) onto those calls and adds
//! the one thing v1 kept in `apps/server` — what a link *permits*.
//!
//! ## Where the scope check actually lives, and why `scope` is stubbed
//!
//! `apps/server/src/share_guest.rs` is the existing guest lane, and it
//! shows what a guest is here: a connection bound to one [`Review`] at
//! upgrade, with the token re-resolved from `ShareStore` on **every**
//! call so disabling, expiring, re-passwording or revoking a link cuts
//! off an already-connected guest at their next request.
//!
//! Two consequences for this lane:
//!
//! - **No token reaches it.** [`crate::backend::FilesBackend`] holds no
//!   session and no method on this trait carries one — the same identity
//!   gap [`super::access`] documents at `this_principal`. So
//!   [`ReviewService::scope`] cannot answer: it is asked "what does *my*
//!   link permit" by a caller this lane cannot name. It faults rather
//!   than guessing, and the resolution it *would* perform is written out
//!   as [`FilesBackend::guest_scope`], which takes the link explicitly —
//!   the shape a dispatcher holding the token will call, exactly as
//!   `authorise`/`effective_for` are in the access lane.
//! - **Per-call scoping is the mount's job, not the method's.** A guest
//!   reaching [`ReviewService::review`] has already been bound to one
//!   review by the wrapper that mounted this lane, and that wrapper
//!   re-resolves the link per call. Re-deriving the binding here from
//!   state this lane cannot see would be a second, weaker copy of a
//!   check that is structural where it lives.
//!
//! No durable state is declared here on purpose. The share store is
//! `apps/server`'s, and a second per-org table of links would be a rival
//! source of truth for revocation — the one thing that must have exactly
//! one. (This is also why nothing here is a module-level static: a
//! process-wide static is shared by every org on the server, which was a
//! real cross-org leak in another lane. There is simply nothing this lane
//! needs to keep.)
//!
//! ## Withheld download means a proxy, not the source
//!
//! `can_download == false` is not a UI affordance; it is a rule about
//! which bytes are served. [`served_rendition`] is that rule, and it is
//! deliberately a total function of the scope so it cannot be forgotten
//! at a call site. [`ReviewService::playback`] cannot honour it yet — the
//! byte lane that redeems a [`ByteTicket`] does not exist — and it faults
//! rather than minting a token nothing can redeem, and rather than
//! quietly handing back source bytes.
//!
//! FUTURE: when the byte lane lands, `playback` resolves the review's
//! file, asks [`served_rendition`] which bytes this scope may have, and
//! mints the ticket for a rendition or for the source accordingly.

use files_proto::error::FilesFault;
use files_proto::id::{CommentId, ReviewId, RootId, VersionId};
use files_proto::model::{NewReviewComment, RenditionKind, Review, ReviewComment};
use files_proto::path::RootPath;
use files_proto::service::access::{Capability, ShareLink};
use files_proto::service::legacy::{FilesError, FilesService};
use files_proto::service::media::{ByteTicket, Region};
use files_proto::service::review::{GuestScope, NewComment, ReviewService};

use crate::backend::FilesBackend;

/// The v1 error onto the v2 fault, for the legacy methods this lane
/// delegates to.
///
/// A free function rather than a `From` impl: both types belong to
/// `files-proto`, so the impl would be orphaned here, and it belongs
/// beside the `From<Error>` in [`crate::error`] whose mapping this
/// mirrors exactly — `NotFound` is prose about *what* was missing and
/// carries no typed id, so it lands as `Invalid` there and here.
///
/// FUTURE: fold this into `files-proto` as `impl From<FilesError> for
/// FilesFault` and delete it, when the legacy surface is retired.
fn fault(err: FilesError) -> FilesFault {
    match err {
        FilesError::NotFound(m) | FilesError::BadRequest(m) => FilesFault::Invalid(m),
        FilesError::AlreadyExists(m) => FilesFault::AlreadyRoot(m),
        FilesError::Io(m) => FilesFault::Io(m),
    }
}

// ── Rules that hold whether or not a caller remembers them ─────────

/// Which bytes a scope may be served: `None` is the source itself.
///
/// A guest without download gets a proxy — never the master. Written as
/// a function of the scope rather than as a branch inside `playback`
/// because it is the *rule*, and a rule spelled once at the only place
/// that can serve bytes is a rule that cannot be skipped by the next
/// method that learns to serve them.
///
/// [`RenditionKind::Proxy720`] rather than `Proxy1080`: the point of
/// withholding download is that what leaves the server is not a usable
/// master, and the smaller ladder rung is the conservative reading. A
/// reviewer watching for notes does not need the larger one.
// t[impl files.scale.large-media] — bytes leave by ticket on the byte
// lane, never inline on this surface
#[must_use]
// t[impl files.review.scope] — the withheld-download rule, as a
// property of the scope rather than a check each method remembers
pub fn served_rendition(scope: &GuestScope) -> Option<RenditionKind> {
    if scope.can_download {
        None
    } else {
        Some(RenditionKind::Proxy720)
    }
}

/// The guest capability set a link conveys, as this lane reads it.
///
/// `Comment` and `Download` are the two capabilities that mean anything
/// to a guest; `Read` is implied by holding the link at all. Everything
/// else a [`ShareLink`] can carry — `Write`, `Deposit`, `Share` — is not
/// reachable from the guest surface, so it is not projected: a scope that
/// advertised a capability with no method behind it would be a lie a
/// client would build a button from.
fn scope_of(review: ReviewId, link: &ShareLink) -> GuestScope {
    GuestScope {
        review,
        can_comment: link.capabilities.contains(&Capability::Comment),
        can_download: link.capabilities.contains(&Capability::Download),
        expires_at: link.expires_at,
    }
}

/// Where in the media a comment sits, in the seconds v1's store records.
///
/// [`Region`] is one addressing scheme shared by search hits, review
/// annotations and resource annotations, and this is the narrowing onto
/// what [`ReviewComment`] can actually hold. `Page` and `Bytes` are
/// refused rather than flattened to zero: a comment on page 4 of a PDF
/// silently landing at 0:00 of it is worse than being told the model
/// cannot carry it yet.
// t[impl files.index.regions] — one region scheme, and review annotations
// are one of its three consumers
fn timecode_of(region: &Region) -> Result<f64, FilesFault> {
    #[allow(clippy::cast_precision_loss)]
    let secs = |ms: u64| ms as f64 / 1000.0;
    match *region {
        Region::Time { start_ms, .. } => Ok(secs(start_ms)),
        // A drawing over a frame is anchored at that frame; over a still
        // there is no time, and the start is the only honest answer.
        Region::Rect { at_ms, .. } => Ok(at_ms.map_or(0.0, secs)),
        Region::Whole => Ok(0.0),
        Region::Page { page } => Err(FilesFault::invalid(format!(
            "a review comment anchors to a moment in the media, and page {page} is not one yet"
        ))),
        Region::Bytes { start, end } => Err(FilesFault::invalid(format!(
            "a review comment anchors to a moment in the media, and bytes {start}..{end} are not one"
        ))),
    }
}

/// The display name a guest comment is posted under.
///
/// Constrained at the boundary, the same way `share_guest.rs` constrains
/// it: whatever the visitor typed, an anonymous link holder must not be
/// able to post as an org member. This lane is the guest lane, so the
/// suffix belongs to every comment that arrives through it.
fn guest_author(given: &str) -> String {
    let name = given.trim();
    if name.is_empty() {
        "Guest".to_string()
    } else {
        format!("{name} (guest)")
    }
}

/// What a comment arriving on this lane is stamped with.
///
/// `ReviewComment::via_link` empty means "an org member wrote this", so
/// leaving it empty for a guest would be a worse lie than a label that is
/// merely imprecise. The precise label — `"<link label> (<token prefix>)"`
/// — needs the link, which does not reach the trait methods.
///
/// FUTURE: thread the resolved link through, and stamp its label.
const VIA_GUEST_LINK: &str = "guest link";

// ── The surface a dispatcher holding the link calls ────────────────

impl FilesBackend {
    // t[impl files.access.internal-sharing] — a link conveys an explicit
    // capability set and nothing beyond it
    /// [`ReviewService::scope`] for an explicit link — the shape the
    /// trait method will take once the token reaches it.
    ///
    /// Resolved per call rather than cached, which is what makes
    /// revocation retroactive mid-connection: `share_guest.rs` re-resolves
    /// the token on every request for exactly this reason, and a scope
    /// answered from a snapshot taken at upgrade would outlive the link
    /// it describes.
    ///
    /// A link addressing a file with no review yet is refused rather than
    /// creating one: minting vault entities is the member side's act
    /// ([`ReviewService::for_file`]), and a guest lane that could create
    /// pages is a guest lane that can write to the vault.
    pub async fn guest_scope(&self, link: &ShareLink) -> Result<GuestScope, FilesFault> {
        let path = link.path.validate()?;
        if link.disabled {
            return Err(FilesFault::denied("Read", path));
        }
        if link.expires_at.is_some_and(|at| at <= chrono::Utc::now()) {
            return Err(FilesFault::denied("Read", path));
        }
        crate::lane::root_or_fault(self, link.root_id)?;

        let this = self.clone();
        let root_id = link.root_id.get();
        let file_path = path.as_str().to_string();
        let found =
            crate::lane::blocking(move || this.find_review_inner(root_id, &file_path)).await?;
        let review = found.ok_or_else(|| {
            FilesFault::invalid(format!(
                "this link addresses {path}, which has no review to open"
            ))
        })?;
        Ok(scope_of(ReviewId::new(review.id), link))
    }
}

// ── The lane ───────────────────────────────────────────────────────

impl ReviewService for FilesBackend {
    /// Faults: no link reaches this lane. See the module doc, and
    /// [`FilesBackend::guest_scope`] for the resolution itself.
    async fn scope(&self) -> Result<GuestScope, FilesFault> {
        Err(FilesFault::Internal(
            "not yet implemented: the link that authorises this lane does not reach it — \
             call FilesBackend::guest_scope with the resolved link"
                .into(),
        ))
    }

    /// The review by id.
    ///
    /// Not scoped here: the mount that bound this connection to one
    /// review is what refuses another id, and it re-resolves the link per
    /// call to do it. Fetched by scanning rather than by a keyed lookup
    /// because the store's keyed read is private to the backend; a review
    /// list is a handful of rows per org.
    async fn review(&self, review: ReviewId) -> Result<Review, FilesFault> {
        let all = FilesService::list_reviews(self, None)
            .await
            .map_err(fault)?;
        all.into_iter()
            .find(|r| r.id == review.get())
            .ok_or(FilesFault::ReviewNotFound(review))
    }

    /// Faults: the byte lane does not exist yet.
    ///
    /// Deliberately not a minted token. A ticket nothing can redeem is
    /// indistinguishable to a client from a working one until the moment
    /// it is used, and the failure would surface as a broken player
    /// rather than as an unimplemented method. The rule this must honour
    /// when it lands is [`served_rendition`], which is already written
    /// and pinned.
    async fn playback(
        &self,
        _review: ReviewId,
        _version: VersionId,
    ) -> Result<ByteTicket, FilesFault> {
        Err(FilesFault::Internal(
            "not yet implemented: the byte lane".into(),
        ))
    }

    /// Comments, ordered by where they sit in the media and, within one
    /// moment, oldest first — so a thread of replies at 0:12 reads down
    /// the page and the newest is last.
    async fn comments(&self, review: ReviewId) -> Result<Vec<ReviewComment>, FilesFault> {
        // Existence first: an unknown id must not read as a review with
        // nothing said about it yet.
        self.review(review).await?;
        FilesService::review_comments(self, review.get())
            .await
            .map_err(fault)
    }

    // t[impl files.index.regions]
    /// Leave a comment against the version being watched.
    ///
    /// The version is carried through as the commit it addresses, and the
    /// backend validates it against the root's store and normalises it to
    /// the store's full spelling — which is what makes the comment belong
    /// to *that* version rather than to whatever is current when it is
    /// read back.
    // t[impl files.review.version-anchored] — the comment records the
    // version being watched, and refuses a region it cannot anchor
    async fn comment(&self, comment: NewComment) -> Result<ReviewComment, FilesFault> {
        let NewComment {
            review,
            version,
            region,
            body,
            strokes,
            author,
        } = comment;
        let timecode_secs = timecode_of(&region)?;
        let added = self
            .add_review_comment_via(
                review.get(),
                NewReviewComment {
                    timecode_secs,
                    author: guest_author(&author),
                    body,
                    // A `VersionId` is the commit's leading 128 bits, and
                    // its 32-hex spelling is a prefix the store's resolver
                    // accepts directly — so no side table is needed to get
                    // from the wire id back to the version.
                    commit_id: version.commit_prefix(),
                    annotation: strokes,
                },
                VIA_GUEST_LINK.to_string(),
            )
            .await
            .map_err(fault)?;
        Ok(added)
    }

    /// Faults: whose comment it is cannot be established here.
    ///
    /// "One's own" needs an identity, and a guest has none that reaches
    /// this lane — the link identifies a *review*, not a person, and two
    /// visitors holding the same link are indistinguishable. Implementing
    /// this on the id alone would let any link holder delete an org
    /// member's feedback, which is why `share_guest.rs` refuses the call
    /// outright rather than narrowing it.
    ///
    /// FUTURE: a per-visitor token stamped onto the comment at creation
    /// would make "one's own" checkable without an account.
    // t[impl files.review.anonymity] — a link identifies a review, not a
    // person, so no guest may remove feedback
    async fn delete_comment(&self, _comment: CommentId) -> Result<ReviewComment, FilesFault> {
        Err(FilesFault::Internal(
            "not yet implemented: comment ownership — no guest identity reaches this lane".into(),
        ))
    }

    /// Get-or-create the review for a file. **The member side's entry
    /// point**, not the guest's: it writes a vault page on first ask, and
    /// the guest surface above only ever reads reviews that already
    /// exist.
    async fn for_file(&self, root_id: RootId, path: RootPath) -> Result<Review, FilesFault> {
        // Checked here so a missing root reads as `RootNotFound` rather
        // than as prose from the legacy path.
        crate::lane::root_or_fault(self, root_id)?;
        let path = path.validate()?;
        FilesService::review_for_file(self, root_id.get(), path.as_str().to_string())
            .await
            .map_err(fault)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, Utc};
    use files_proto::id::{RootId, ShareId};

    use super::*;

    fn link(capabilities: Vec<Capability>) -> ShareLink {
        ShareLink {
            id: ShareId::generate(),
            root_id: RootId::generate(),
            path: RootPath::parse("cut.mov").expect("path"),
            capabilities,
            token: "t".into(),
            password_set: false,
            expires_at: None,
            disabled: false,
        }
    }

    // t[verify files.scale.large-media]
    #[test]
    fn a_guest_without_download_is_served_a_proxy_rather_than_the_source() {
        let review = ReviewId::generate();
        let viewer = scope_of(review, &link(vec![Capability::Read]));
        assert!(!viewer.can_download);
        assert_eq!(
            served_rendition(&viewer),
            Some(RenditionKind::Proxy720),
            "withholding download is a rule about which bytes are served, \
             not a button a client may ignore"
        );

        let downloader = scope_of(review, &link(vec![Capability::Read, Capability::Download]));
        assert!(downloader.can_download);
        assert_eq!(
            served_rendition(&downloader),
            None,
            "and a link that does convey download gets the source"
        );
    }

    // t[verify files.access.internal-sharing]
    #[test]
    fn a_scope_advertises_only_what_the_link_conveys() {
        let review = ReviewId::generate();
        let view_only = scope_of(review, &link(vec![Capability::Read]));
        assert_eq!(view_only.review, review);
        assert!(!view_only.can_comment, "view-only means view-only");

        let commenter = scope_of(review, &link(vec![Capability::Read, Capability::Comment]));
        assert!(commenter.can_comment);

        // Capabilities with no guest method behind them are not projected
        // at all — there is no field for them, and that is the point.
        let over = scope_of(
            review,
            &link(vec![
                Capability::Write,
                Capability::Share,
                Capability::Deposit,
            ]),
        );
        assert!(!over.can_comment && !over.can_download);
    }

    #[test]
    fn an_expiry_rides_the_scope_verbatim() {
        let at = Utc::now() + TimeDelta::hours(2);
        let mut l = link(vec![Capability::Read]);
        l.expires_at = Some(at);
        assert_eq!(scope_of(ReviewId::generate(), &l).expires_at, Some(at));
    }

    // t[verify files.index.regions]
    #[test]
    fn a_region_narrows_to_the_moment_a_comment_records() {
        assert!(
            (timecode_of(&Region::Time {
                start_ms: 12_500,
                end_ms: 13_000
            })
            .unwrap()
                - 12.5)
                .abs()
                < f64::EPSILON
        );
        assert!(
            (timecode_of(&Region::Rect {
                x: 0.1,
                y: 0.2,
                w: 0.3,
                h: 0.4,
                at_ms: Some(4_000)
            })
            .unwrap()
                - 4.0)
                .abs()
                < f64::EPSILON,
            "a drawing is anchored at the frame it was drawn over"
        );
        assert_eq!(timecode_of(&Region::Whole).unwrap(), 0.0);
    }

    // t[verify files.index.regions]
    #[test]
    fn a_region_the_model_cannot_carry_is_refused_rather_than_flattened() {
        // Landing page 4 at 0:00 would attach the note to the wrong part
        // of the document and look like it worked.
        assert!(matches!(
            timecode_of(&Region::Page { page: 4 }),
            Err(FilesFault::Invalid(_))
        ));
        assert!(matches!(
            timecode_of(&Region::Bytes { start: 0, end: 9 }),
            Err(FilesFault::Invalid(_))
        ));
    }

    #[test]
    fn a_guest_cannot_post_under_a_bare_name() {
        assert_eq!(guest_author("Cody"), "Cody (guest)");
        assert_eq!(guest_author("   "), "Guest");
        assert_eq!(
            guest_author("Cody (guest)"),
            "Cody (guest) (guest)",
            "the suffix is applied at the boundary, so a crafted name \
             cannot shed it"
        );
    }
}
