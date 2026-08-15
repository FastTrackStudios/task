//! Signed HTTP URLs for blob upload + download.
//!
//! Format mirrors the capability tokens from Phase 3:
//! `base64url(signature || canonical_message)` over Ed25519. The
//! `canonical_message` is short, hand-rolled, deterministic UTF-8:
//!
//! ```text
//! blob/v1|<purpose>|<expires_unix>|<subject>
//! ```
//!
//! `purpose` is `up` (upload) or `dn` (download). `subject` is the
//! `upload_id` (UUID) for uploads and the `content_hash` (sha256
//! hex) for downloads.
//!
//! Tokens are HTTP-only — they live in the URL's query string, not
//! in vox metadata.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{SIGNATURE_LENGTH, Signature, Signer, Verifier};
use uuid::Uuid;

use crate::capability::{CapabilityError, ServerKeypair};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobTokenPurpose {
    Upload,
    Download,
    /// Read under a colocated-media path PREFIX
    /// (`/org/{slug}/media/{path}`). See [`BlobToken::media`].
    Media,
}

impl BlobTokenPurpose {
    fn tag(&self) -> &'static str {
        match self {
            Self::Upload => "up",
            Self::Download => "dn",
            Self::Media => "md",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "up" => Some(Self::Upload),
            "dn" => Some(Self::Download),
            "md" => Some(Self::Media),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobToken {
    pub purpose: BlobTokenPurpose,
    pub expires_unix: i64,
    /// `upload_id` (UUID) for uploads, `content_hash` (sha256 hex)
    /// for downloads.
    pub subject: String,
}

impl BlobToken {
    #[must_use]
    pub fn upload(upload_id: Uuid, expires_unix: i64) -> Self {
        Self {
            purpose: BlobTokenPurpose::Upload,
            expires_unix,
            subject: upload_id.to_string(),
        }
    }
    pub fn download(content_hash: impl Into<String>, expires_unix: i64) -> Self {
        Self {
            purpose: BlobTokenPurpose::Download,
            expires_unix,
            subject: content_hash.into(),
        }
    }

    /// A **prefix** grant over `/org/{slug}/media/…`.
    ///
    /// Prefix rather than one-file, because the player loads a song as a
    /// directory: manifest, chart, and N stems all under
    /// `songs/<song>/`. Minting per file would be an RPC per stem on
    /// every load, and would have to happen inside `<audio>.set_src`,
    /// which is synchronous. One short-lived token per song load covers
    /// the set.
    ///
    /// The subject binds the ORG as well as the path — `{slug}/{prefix}`
    /// — so a token minted for one org can never read another's files.
    /// Slugs contain no `/`, so the join is unambiguous.
    pub fn media(slug: &str, prefix: &str, expires_unix: i64) -> Self {
        Self {
            purpose: BlobTokenPurpose::Media,
            expires_unix,
            subject: format!("{slug}/{}", prefix.trim_start_matches('/')),
        }
    }

    /// Does this token authorize reading `rel` in org `slug`?
    ///
    /// Compares the FULL canonical subject rather than parsing it apart,
    /// so a `/` inside a path can never be mistaken for the org
    /// separator. Prefix match is on a path-segment boundary: a grant
    /// for `songs/hymn` must not unlock `songs/hymn-outtakes`.
    #[must_use]
    pub fn allows_media(&self, slug: &str, rel: &str) -> bool {
        if !matches!(self.purpose, BlobTokenPurpose::Media) {
            return false;
        }
        let requested = format!("{slug}/{}", rel.trim_start_matches('/'));
        let granted = self.subject.trim_end_matches('/');
        requested == granted
            || requested
                .strip_prefix(granted)
                .is_some_and(|rest| rest.starts_with('/'))
    }

    #[must_use]
    pub fn upload_id(&self) -> Option<Uuid> {
        if matches!(self.purpose, BlobTokenPurpose::Upload) {
            Uuid::parse_str(&self.subject).ok()
        } else {
            None
        }
    }
    #[must_use]
    pub fn content_hash(&self) -> Option<&str> {
        if matches!(self.purpose, BlobTokenPurpose::Download) {
            Some(self.subject.as_str())
        } else {
            None
        }
    }

    fn canonical_message(&self) -> String {
        format!(
            "blob/v1|{}|{}|{}",
            self.purpose.tag(),
            self.expires_unix,
            self.subject,
        )
    }

    /// Sign + encode this token. The string is opaque to clients.
    #[must_use]
    pub fn issue(&self, kp: &ServerKeypair) -> String {
        kp.issue_blob_token(self)
    }

    /// Verify a token string against `now_unix`. Errors are
    /// folded into [`CapabilityError`] so the WS handler + this
    /// path share one error type.
    pub fn verify(token: &str, kp: &ServerKeypair, now_unix: i64) -> Result<Self, CapabilityError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| CapabilityError::BadBase64)?;
        if bytes.len() < SIGNATURE_LENGTH {
            return Err(CapabilityError::Malformed);
        }
        let (sig_bytes, msg_bytes) = bytes.split_at(SIGNATURE_LENGTH);
        let sig_arr: [u8; SIGNATURE_LENGTH] = sig_bytes
            .try_into()
            .map_err(|_| CapabilityError::Malformed)?;
        let sig = Signature::from_bytes(&sig_arr);
        kp.verifying_key()
            .verify(msg_bytes, &sig)
            .map_err(|_| CapabilityError::BadSignature)?;
        let msg = std::str::from_utf8(msg_bytes).map_err(|_| CapabilityError::Malformed)?;
        let parsed = Self::parse_message(msg)?;
        if now_unix >= parsed.expires_unix {
            return Err(CapabilityError::Expired);
        }
        Ok(parsed)
    }

    fn parse_message(s: &str) -> Result<Self, CapabilityError> {
        let mut parts = s.splitn(4, '|');
        let v = parts.next().ok_or(CapabilityError::Malformed)?;
        if v != "blob/v1" {
            return Err(CapabilityError::UnsupportedVersion);
        }
        let purpose_tag = parts.next().ok_or(CapabilityError::Malformed)?;
        let purpose = BlobTokenPurpose::parse(purpose_tag).ok_or(CapabilityError::Malformed)?;
        let expires_unix: i64 = parts
            .next()
            .ok_or(CapabilityError::Malformed)?
            .parse()
            .map_err(|_| CapabilityError::Malformed)?;
        let subject = parts.next().ok_or(CapabilityError::Malformed)?.to_string();
        Ok(Self {
            purpose,
            expires_unix,
            subject,
        })
    }
}

// Sign helper sits on `ServerKeypair` so callers don't reach into
// the private SigningKey field. Module-private — exposed via
// `BlobToken::issue`.
impl ServerKeypair {
    pub(crate) fn issue_blob_token(&self, token: &BlobToken) -> String {
        let msg = token.canonical_message();
        let sig: Signature = self.signing_key().sign(msg.as_bytes());
        let mut out = Vec::with_capacity(SIGNATURE_LENGTH + msg.len());
        out.extend_from_slice(&sig.to_bytes());
        out.extend_from_slice(msg.as_bytes());
        URL_SAFE_NO_PAD.encode(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn future() -> i64 {
        i64::MAX / 2
    }

    #[test]
    fn upload_token_round_trips() {
        let kp = ServerKeypair::generate_ephemeral();
        let id = Uuid::new_v4();
        let tok = BlobToken::upload(id, future());
        let s = tok.issue(&kp);
        let back = BlobToken::verify(&s, &kp, 0).expect("verify");
        assert_eq!(back, tok);
        assert_eq!(back.upload_id(), Some(id));
        assert_eq!(back.content_hash(), None);
    }

    #[test]
    fn download_token_round_trips() {
        let kp = ServerKeypair::generate_ephemeral();
        let hash = "deadbeef0123";
        let tok = BlobToken::download(hash, future());
        let s = tok.issue(&kp);
        let back = BlobToken::verify(&s, &kp, 0).expect("verify");
        assert_eq!(back, tok);
        assert_eq!(back.content_hash(), Some(hash));
    }

    #[test]
    fn expired_rejected() {
        let kp = ServerKeypair::generate_ephemeral();
        let tok = BlobToken::download("x", 100);
        let s = tok.issue(&kp);
        match BlobToken::verify(&s, &kp, 200) {
            Err(CapabilityError::Expired) => {}
            other => panic!("expected Expired, got {other:?}"),
        }
    }

    #[test]
    fn wrong_keypair_rejected() {
        let kp_a = ServerKeypair::generate_ephemeral();
        let kp_b = ServerKeypair::generate_ephemeral();
        let tok = BlobToken::download("x", future());
        let s = tok.issue(&kp_a);
        assert!(matches!(
            BlobToken::verify(&s, &kp_b, 0),
            Err(CapabilityError::BadSignature)
        ));
    }

    #[test]
    fn tampered_subject_rejected() {
        let kp = ServerKeypair::generate_ephemeral();
        let tok = BlobToken::download("good-hash", future());
        let s = tok.issue(&kp);
        let mut bytes = URL_SAFE_NO_PAD.decode(&s).unwrap();
        // Flip a byte INSIDE the message portion.
        let i = bytes.len() - 1;
        bytes[i] ^= 0x01;
        let bad = URL_SAFE_NO_PAD.encode(bytes);
        assert!(matches!(
            BlobToken::verify(&bad, &kp, 0),
            Err(CapabilityError::BadSignature)
        ));
    }

    // ── media prefix grants ──────────────────────────────────────────

    #[test]
    fn media_grant_covers_its_prefix() {
        let tok = BlobToken::media("codywright", "songs/hymn", future());
        assert!(tok.allows_media("codywright", "songs/hymn/manifest.json"));
        assert!(tok.allows_media("codywright", "songs/hymn/stems/bass.ogg"));
        // The prefix itself (the directory listing) is in scope too.
        assert!(tok.allows_media("codywright", "songs/hymn"));
    }

    #[test]
    fn media_grant_does_not_leak_across_orgs() {
        // THE property: the subject binds the org, so a token minted for
        // one org can never read another's files even at the same path.
        let tok = BlobToken::media("codywright", "songs/hymn", future());
        assert!(!tok.allows_media("tombrooksmusic", "songs/hymn/manifest.json"));
    }

    #[test]
    fn media_prefix_matches_on_a_segment_boundary() {
        // A grant for `songs/hymn` must not unlock `songs/hymn-outtakes`
        // — the classic string-prefix bug in a path grant.
        let tok = BlobToken::media("codywright", "songs/hymn", future());
        assert!(!tok.allows_media("codywright", "songs/hymn-outtakes/master.wav"));
        assert!(!tok.allows_media("codywright", "songs/hymnal/secret.wav"));
    }

    #[test]
    fn media_grant_does_not_escape_upward() {
        let tok = BlobToken::media("codywright", "songs/hymn", future());
        assert!(!tok.allows_media("codywright", "songs"));
        assert!(!tok.allows_media("codywright", "other/file.wav"));
    }

    #[test]
    fn a_download_token_is_not_a_media_token() {
        // Purpose confusion: a blob-download grant must not become a
        // filesystem read grant just because the subject string lines up.
        let tok = BlobToken::download("codywright/songs/hymn", future());
        assert!(!tok.allows_media("codywright", "songs/hymn/manifest.json"));
    }

    #[test]
    fn media_token_round_trips_and_expires() {
        let kp = ServerKeypair::generate_ephemeral();
        let tok = BlobToken::media("codywright", "songs/hymn", future());
        let s = tok.issue(&kp);
        let back = BlobToken::verify(&s, &kp, 0).expect("verify");
        assert!(back.allows_media("codywright", "songs/hymn/x.ogg"));
        assert!(matches!(
            BlobToken::verify(&s, &kp, future() + 1),
            Err(CapabilityError::Expired)
        ));
    }
}
