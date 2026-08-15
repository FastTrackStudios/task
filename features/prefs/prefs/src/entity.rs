//! Re-exports of the architect-emitted SeaORM items.

pub use prefs_proto::user_prefs::__user_prefs_storage::{
    ActiveModel as UserPrefsActive, Column as UserPrefsColumn, Entity as UserPrefsEntity,
    Model as UserPrefsModel, Relation as UserPrefsRelation,
};

// Repo + storage glue.
pub use prefs_proto::UserPrefs;
pub use prefs_proto::user_prefs::{
    UserPrefsCreate, UserPrefsRepo, UserPrefsRepoStorage, UserPrefsUpdate,
};
