#![allow(clippy::large_futures)]
//! Email migration against a REAL sqlite auth store.
//!
//! The `auth` crate's own tests cover this flow with in-memory storage,
//! which never touches sea-orm or the migrator — the gap that let a
//! duplicate migration name through until the server's boot tests caught
//! it. So this exercises the parts only a real database can fail at: that
//! the `auth_user_email_history` table exists after migrations run, that
//! the sea-orm storage impl reads and writes it, and that a live
//! `AuthState` (the same one the server opens) behaves.

use architect_auth::{CreateEmailPasswordUser, MigrateUserEmail};
use task_server::AuthState;

/// A real on-disk auth store, migrated exactly as the server does it.
async fn store() -> eyre::Result<(AuthState, tempfile::TempDir)> {
    let tmp = tempfile::tempdir()?;
    let db = tmp.path().join("auth.sqlite");
    let auth = AuthState::open(
        &format!("sqlite://{}?mode=rwc", db.display()),
        "test-secret-at-least-32-bytes!!!",
    )
    .await?;
    Ok((auth, tmp))
}

async fn seed(auth: &AuthState, email: &str) -> eyre::Result<uuid::Uuid> {
    let bundle = auth
        .auth
        .create_email_password_user(CreateEmailPasswordUser {
            email: email.into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Seed".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("seed `{email}`: {e:?}"))?;
    Ok(bundle.user.id)
}

#[tokio::test(flavor = "multi_thread")]
async fn migration_persists_and_records_against_real_sqlite() -> eyre::Result<()> {
    let (auth, _tmp) = store().await?;
    let id = seed(&auth, "old@example.test").await?;

    let moved = auth
        .auth
        .migrate_user_email(MigrateUserEmail {
            user_id: id,
            new_email: "new@example.test".into(),
            changed_by: None,
            reason: Some("operator migration".into()),
        })
        .await
        .map_err(|e| eyre::eyre!("migrate: {e:?}"))?;

    // THE property: the id is what everything else is keyed on.
    assert_eq!(moved.id, id, "migration must not change the user id");
    assert_eq!(moved.email.as_deref(), Some("new@example.test"));
    assert!(!moved.email_verified, "a new address starts unverified");

    // The row actually landed in sqlite — not just in a Vec somewhere.
    let history = auth
        .auth
        .list_email_history(id)
        .await
        .map_err(|e| eyre::eyre!("history: {e:?}"))?;
    assert_eq!(history.len(), 1, "expected one row, got {history:?}");
    assert_eq!(
        history[0].previous_email.as_deref(),
        Some("old@example.test")
    );
    assert_eq!(history[0].new_email, "new@example.test");
    assert_eq!(
        history[0].changed_by, None,
        "an operator migration records no signed-in user"
    );

    // Signing in with the NEW address works…
    auth.auth
        .sign_in_email_password(architect_auth::SignInEmailPassword {
            email: "new@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("sign in with the migrated address: {e:?}"))?;

    // …and the old one no longer authenticates, but is still ATTRIBUTABLE.
    // Both halves matter: the address is released, the record is not.
    let old_login = auth
        .auth
        .sign_in_email_password(architect_auth::SignInEmailPassword {
            email: "old@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            ip_address: None,
            user_agent: None,
        })
        .await;
    assert!(
        old_login.is_err(),
        "the old address must stop authenticating"
    );

    let who = auth
        .auth
        .find_user_by_previous_email("old@example.test")
        .await
        .map_err(|e| eyre::eyre!("reverse lookup: {e:?}"))?
        .expect("the old address should still resolve to its account");
    assert_eq!(who.id, id);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn migration_is_idempotent_and_refuses_collisions_on_real_sqlite() -> eyre::Result<()> {
    let (auth, _tmp) = store().await?;
    let id = seed(&auth, "mine@example.test").await?;
    seed(&auth, "theirs@example.test").await?;

    // Onto an address someone else holds: refused, and no row written.
    let clash = auth
        .auth
        .migrate_user_email(MigrateUserEmail {
            user_id: id,
            new_email: "theirs@example.test".into(),
            changed_by: None,
            reason: None,
        })
        .await;
    assert!(
        clash.is_err(),
        "must not merge two accounts onto one address"
    );
    assert!(
        auth.auth.list_email_history(id).await.unwrap().is_empty(),
        "a refused migration must leave no trace"
    );

    // Onto the address already held: a no-op, so re-running a bulk
    // migration after a partial failure is safe.
    auth.auth
        .migrate_user_email(MigrateUserEmail {
            user_id: id,
            new_email: "mine@example.test".into(),
            changed_by: None,
            reason: None,
        })
        .await
        .map_err(|e| eyre::eyre!("same-address migrate should succeed: {e:?}"))?;
    assert!(
        auth.auth.list_email_history(id).await.unwrap().is_empty(),
        "a no-op must not append a row claiming a change"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn operator_verbs_manage_users_without_a_session() -> eyre::Result<()> {
    // The operator path exists precisely for accounts nobody can sign in
    // as, so none of this may require a session or an admin.
    let (auth, _tmp) = store().await?;
    let keep = seed(&auth, "keep@example.test").await?;
    let drop = seed(&auth, "drop@example.test").await?;

    let users = auth.auth.list_users_local_trusted().await.unwrap();
    assert_eq!(users.len(), 2, "both seeded accounts should list");

    // Reset a password with no old password and no admin.
    auth.auth
        .set_user_password_local_trusted(keep, "a-brand-new-correct-horse")
        .await
        .map_err(|e| eyre::eyre!("set password: {e:?}"))?;
    auth.auth
        .sign_in_email_password(architect_auth::SignInEmailPassword {
            email: "keep@example.test".into(),
            password: "a-brand-new-correct-horse".into(),
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("sign in with the reset password: {e:?}"))?;

    // …and the OLD password must stop working, or a reset isn't a reset.
    let old = auth
        .auth
        .sign_in_email_password(architect_auth::SignInEmailPassword {
            email: "keep@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            ip_address: None,
            user_agent: None,
        })
        .await;
    assert!(old.is_err(), "the superseded password must stop working");

    // Delete removes it, and a second delete is an error rather than a
    // silent success — "did that do anything?" is the operator's question.
    auth.auth
        .delete_user_local_trusted(drop)
        .await
        .map_err(|e| eyre::eyre!("delete: {e:?}"))?;
    assert!(
        auth.auth.delete_user_local_trusted(drop).await.is_err(),
        "deleting an already-deleted account must report that, not succeed"
    );
    let after = auth.auth.list_users_local_trusted().await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].email.as_deref(), Some("keep@example.test"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_role_can_be_seeded_without_an_existing_admin() -> eyre::Result<()> {
    // The bootstrap property: `require_admin` needs an admin, so the
    // FIRST admin cannot be made through the admin flows. If this path
    // needed one too, an org could never get its first.
    let (auth, _tmp) = store().await?;
    let id = seed(&auth, "owner@example.test").await?;

    let before = auth.auth.list_users_local_trusted().await.unwrap();
    assert!(
        before[0].role.as_deref() != Some("admin"),
        "a freshly seeded user should not already be admin"
    );

    let promoted = auth
        .auth
        .set_user_role_local_trusted(id, Some("admin".into()))
        .await
        .map_err(|e| eyre::eyre!("grant admin: {e:?}"))?;
    assert_eq!(promoted.role.as_deref(), Some("admin"));

    // …and it can be taken away again, or a mistake would be permanent.
    let cleared = auth
        .auth
        .set_user_role_local_trusted(id, None)
        .await
        .map_err(|e| eyre::eyre!("clear role: {e:?}"))?;
    assert_eq!(cleared.role, None);

    Ok(())
}
