//! get-returns-defaults + set-then-get round trip.

use prefs::{Migrator, Store};
use prefs_proto::UserPrefs;
use prefs_proto::service::PrefsService;
use sea_orm::Database;
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

async fn new_store() -> Store {
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    Store::new(conn)
}

#[tokio::test]
async fn get_without_row_returns_defaults() {
    let store = new_store().await;
    let user = Uuid::new_v4();
    let prefs = store.get(user).await.unwrap();
    assert_eq!(prefs, UserPrefs::defaults_for(user));
    assert!(prefs.tasks_active, "tasks.active defaults on");
    assert!(prefs.tasks_relevant, "tasks.relevant defaults on");
    assert_eq!(prefs.default_page, "");
    assert_eq!(prefs.location, "");
    assert!(prefs.shortcuts_priority, "app shortcuts win by default");
}

#[tokio::test]
async fn set_then_get_round_trips() {
    let store = new_store().await;
    let user = Uuid::new_v4();
    let wanted = UserPrefs {
        user_id: user,
        default_page: "/tasks".into(),
        tasks_active: false,
        tasks_relevant: true,
        location: "studio".into(),
        theme_preset: "rose-pine".into(),
        theme_mode: "dark".into(),
        shortcuts_priority: false,
    };
    let stored = store.set(wanted.clone()).await.unwrap();
    assert_eq!(stored, wanted);
    assert_eq!(store.get(user).await.unwrap(), wanted);
}

#[tokio::test]
async fn second_set_upserts_in_place() {
    let store = new_store().await;
    let user = Uuid::new_v4();
    store.set(UserPrefs::defaults_for(user)).await.unwrap();
    let updated = UserPrefs {
        user_id: user,
        default_page: "/calendar".into(),
        tasks_active: true,
        tasks_relevant: false,
        location: "home".into(),
        theme_preset: "nord".into(),
        theme_mode: "light".into(),
        shortcuts_priority: true,
    };
    store.set(updated.clone()).await.unwrap();
    assert_eq!(store.get(user).await.unwrap(), updated);
}

#[tokio::test]
async fn prefs_are_per_user() {
    let store = new_store().await;
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let a_prefs = UserPrefs {
        user_id: a,
        default_page: "/tasks".into(),
        tasks_active: false,
        tasks_relevant: false,
        location: "studio".into(),
        theme_preset: String::new(),
        theme_mode: String::new(),
        shortcuts_priority: false,
    };
    store.set(a_prefs.clone()).await.unwrap();
    assert_eq!(store.get(a).await.unwrap(), a_prefs);
    // b never wrote — still defaults.
    assert_eq!(store.get(b).await.unwrap(), UserPrefs::defaults_for(b));
}
