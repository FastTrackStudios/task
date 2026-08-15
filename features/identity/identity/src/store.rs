//! Server-side identity-locker store over SeaORM.
//!
//! Holds, per home-org user, the encrypted session tokens for other
//! servers they've linked. Tokens are encrypted at rest with
//! `auth::crypto::encrypt_secret` (a `v2.<keyid>.<nonce>.<ct>`
//! envelope) and only decrypted on read.

use auth::crypto::{decrypt_secret, encrypt_secret};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use crate::entity::{
    LinkedServerActive, LinkedServerColumn, LinkedServerEntity, LinkedServerModel,
};
use crate::error::IdentityDbError;

/// A linked-server record with its token in **plaintext** — the
/// decrypted view the store hands back and accepts. The token is only
/// ever plaintext in memory; at rest it lives as
/// [`crate::entity::LinkedServer::token_ciphertext`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRecord {
    pub id: Uuid,
    pub home_user_id: Uuid,
    pub label: String,
    pub remote_url: String,
    pub remote_slug: String,
    pub remote_user_id: Option<Uuid>,
    pub remote_email: Option<String>,
    /// Plaintext session token. `None` = leave the stored token as-is
    /// on upsert (or absent).
    pub token: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Clone)]
pub struct Store {
    conn: DatabaseConnection,
    secret: String,
}

impl Store {
    /// Construct from an already-opened connection and the AEAD secret
    /// used to (de)crypt stored tokens. The caller must run
    /// [`crate::Migrator`] before invoking any method.
    #[must_use]
    pub fn new(conn: DatabaseConnection, secret: String) -> Self {
        Self { conn, secret }
    }

    #[must_use]
    pub fn conn(&self) -> &DatabaseConnection {
        &self.conn
    }

    /// All links owned by `home_user_id`, tokens decrypted. A decrypt
    /// failure on any row is surfaced as an error, never silently
    /// dropped.
    pub async fn list_links(&self, home_user_id: Uuid) -> Result<Vec<LinkRecord>, IdentityDbError> {
        let rows = LinkedServerEntity::find()
            .filter(LinkedServerColumn::HomeUserId.eq(home_user_id))
            .all(&self.conn)
            .await?;
        rows.into_iter().map(|m| self.model_to_record(m)).collect()
    }

    /// Insert or update a link, keyed by
    /// `(home_user_id, remote_url, remote_slug)`. Encrypts
    /// `rec.token` (if `Some`) into `token_ciphertext`; on update with
    /// `token: None` the stored ciphertext is left untouched. Returns
    /// the stored record with its token decrypted.
    pub async fn upsert_link(&self, rec: LinkRecord) -> Result<LinkRecord, IdentityDbError> {
        let now = Utc::now().timestamp();
        let encrypted = match rec.token.as_deref() {
            Some(token) => Some(encrypt_secret(&self.secret, token)?),
            None => None,
        };

        let existing = LinkedServerEntity::find()
            .filter(LinkedServerColumn::HomeUserId.eq(rec.home_user_id))
            .filter(LinkedServerColumn::RemoteUrl.eq(rec.remote_url.clone()))
            .filter(LinkedServerColumn::RemoteSlug.eq(rec.remote_slug.clone()))
            .one(&self.conn)
            .await?;

        let stored = if let Some(row) = existing {
            let mut active: LinkedServerActive = row.into();
            active.label = Set(rec.label);
            active.remote_user_id = Set(rec.remote_user_id);
            active.remote_email = Set(rec.remote_email);
            if let Some(ciphertext) = encrypted {
                active.token_ciphertext = Set(ciphertext);
            }
            active.expires_at = Set(rec.expires_at);
            active.updated_at = Set(now);
            active.update(&self.conn).await?
        } else {
            let id = if rec.id.is_nil() {
                Uuid::new_v4()
            } else {
                rec.id
            };
            let ciphertext = encrypted.unwrap_or_default();
            LinkedServerActive {
                id: Set(id),
                home_user_id: Set(rec.home_user_id),
                label: Set(rec.label),
                remote_url: Set(rec.remote_url),
                remote_slug: Set(rec.remote_slug),
                remote_user_id: Set(rec.remote_user_id),
                remote_email: Set(rec.remote_email),
                token_ciphertext: Set(ciphertext),
                expires_at: Set(rec.expires_at),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&self.conn)
            .await?
        };

        self.model_to_record(stored)
    }

    /// Delete a link by `id`, scoped to `home_user_id` so a user can't
    /// delete another user's row.
    pub async fn delete_link(&self, home_user_id: Uuid, id: Uuid) -> Result<(), IdentityDbError> {
        LinkedServerEntity::delete_many()
            .filter(LinkedServerColumn::Id.eq(id))
            .filter(LinkedServerColumn::HomeUserId.eq(home_user_id))
            .exec(&self.conn)
            .await?;
        Ok(())
    }

    fn model_to_record(&self, m: LinkedServerModel) -> Result<LinkRecord, IdentityDbError> {
        // An empty ciphertext = a row stored without a token; anything
        // else must decrypt or the read is an error (never silently
        // dropped).
        let token = if m.token_ciphertext.is_empty() {
            None
        } else {
            Some(decrypt_secret(&self.secret, &m.token_ciphertext)?)
        };
        Ok(LinkRecord {
            id: m.id,
            home_user_id: m.home_user_id,
            label: m.label,
            remote_url: m.remote_url,
            remote_slug: m.remote_slug,
            remote_user_id: m.remote_user_id,
            remote_email: m.remote_email,
            token,
            expires_at: m.expires_at,
        })
    }
}
