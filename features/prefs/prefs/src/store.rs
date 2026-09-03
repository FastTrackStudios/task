//! `PrefsService` impl over SeaORM.

use prefs_proto::service::PrefsService;
use prefs_proto::{PrefsError, UserPrefs};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;

use crate::entity::{UserPrefsActive, UserPrefsEntity, UserPrefsModel};
use crate::error::PrefsDbError;

#[derive(Clone)]
pub struct Store {
    conn: DatabaseConnection,
}

impl Store {
    /// Construct from an already-opened connection. The caller
    /// must run [`crate::Migrator`] before invoking any method.
    #[must_use]
    pub fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }

    #[must_use]
    pub fn conn(&self) -> &DatabaseConnection {
        &self.conn
    }

    async fn read(&self, user_id: Uuid) -> Result<Option<UserPrefs>, PrefsDbError> {
        let row = UserPrefsEntity::find_by_id(user_id).one(&self.conn).await?;
        Ok(row.map(model_to_prefs))
    }

    async fn upsert(&self, prefs: UserPrefs) -> Result<UserPrefs, PrefsDbError> {
        let existing = UserPrefsEntity::find_by_id(prefs.user_id)
            .one(&self.conn)
            .await?;
        let row = if let Some(row) = existing {
            let mut active: UserPrefsActive = row.into();
            active.default_page = Set(prefs.default_page);
            active.tasks_active = Set(prefs.tasks_active);
            active.tasks_relevant = Set(prefs.tasks_relevant);
            active.location = Set(prefs.location);
            active.theme_preset = Set(prefs.theme_preset);
            active.theme_mode = Set(prefs.theme_mode);
            active.shortcuts_priority = Set(prefs.shortcuts_priority);
            active.vim_mode = Set(prefs.vim_mode);
            active.update(&self.conn).await?
        } else {
            UserPrefsActive {
                user_id: Set(prefs.user_id),
                default_page: Set(prefs.default_page),
                tasks_active: Set(prefs.tasks_active),
                tasks_relevant: Set(prefs.tasks_relevant),
                location: Set(prefs.location),
                theme_preset: Set(prefs.theme_preset),
                theme_mode: Set(prefs.theme_mode),
                shortcuts_priority: Set(prefs.shortcuts_priority),
                vim_mode: Set(prefs.vim_mode),
            }
            .insert(&self.conn)
            .await?
        };
        Ok(model_to_prefs(row))
    }
}

// ── Trait impl ─────────────────────────────────────────────

impl PrefsService for Store {
    async fn get(&self, user_id: Uuid) -> Result<UserPrefs, PrefsError> {
        let stored = self.read(user_id).await.map_err(PrefsError::from)?;
        Ok(stored.unwrap_or_else(|| UserPrefs::defaults_for(user_id)))
    }

    async fn set(&self, prefs: UserPrefs) -> Result<UserPrefs, PrefsError> {
        self.upsert(prefs).await.map_err(Into::into)
    }
}

fn model_to_prefs(m: UserPrefsModel) -> UserPrefs {
    UserPrefs {
        user_id: m.user_id,
        default_page: m.default_page,
        tasks_active: m.tasks_active,
        tasks_relevant: m.tasks_relevant,
        location: m.location,
        theme_preset: m.theme_preset,
        theme_mode: m.theme_mode,
        shortcuts_priority: m.shortcuts_priority,
        vim_mode: m.vim_mode,
    }
}
