//! [`LinkedServer`] — one row per home-org user per linked remote
//! server, holding the encrypted session token for that server.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq, Eq,
)]
#[architect(table_name = "linked_servers", repo)]
pub struct LinkedServer {
    /// Row id. Not auto-increment — we mint `Uuid::new_v4()`.
    #[architect(primary_key, auto_increment = false, filterable)]
    pub id: Uuid,

    /// The owner — the home-org `AuthUser::id` this link belongs to.
    #[architect(filterable)]
    pub home_user_id: Uuid,

    /// Human label for the linked server.
    pub label: String,

    /// vox base URL of the linked server.
    pub remote_url: String,

    /// Org slug on that server.
    pub remote_slug: String,

    /// The linked account's user id on the remote server, if known.
    pub remote_user_id: Option<Uuid>,

    /// The linked account's email on the remote server, if known.
    pub remote_email: Option<String>,

    /// `auth::crypto::encrypt_secret()` envelope
    /// (`v2.<keyid>.<nonce>.<ct>`). NEVER plaintext.
    pub token_ciphertext: String,

    /// Token expiry, unix seconds. `None` = unknown.
    pub expires_at: Option<i64>,

    /// Row creation time, unix seconds.
    pub created_at: i64,

    /// Row last-update time, unix seconds.
    pub updated_at: i64,
}
