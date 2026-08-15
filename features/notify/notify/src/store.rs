//! `Notify` impl over SeaORM — the per-org notifications queue.

use chrono::Utc;
use notify_proto::{
    Notification, NotifyError, NotifyEvent, NotifyKind, NotifyListFilter, NotifySource,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, QuerySelect, Set,
};
use uuid::Uuid;

use crate::entity::{self, Column, Entity as Notifications};

/// Server default page size for [`notify_proto::Notify::list`].
const DEFAULT_PAGE: u64 = 100;

/// Everything a notifier rule produces — the store mints `id` /
/// `created_at` / `read_at` on push.
#[derive(Debug, Clone)]
pub struct NewNotification {
    pub kind: NotifyKind,
    pub title: String,
    pub body: String,
    pub source: NotifySource,
    pub actor: String,
}

#[derive(Clone)]
pub struct Store {
    conn: DatabaseConnection,
    /// Fan-out hub behind the `#[subscribe] fn events` stream — every
    /// successful mutation publishes the post-write row here
    /// ([`NotifyEvent::Upserted`] / [`NotifyEvent::Deleted`]).
    /// Sliding mailbox: a slow subscriber loses its *oldest* queued
    /// events, which is correct for state-shaped payloads. Clones
    /// share the hub (`Arc` inside), so the service mount, the stream
    /// mount, and the notifier can each hold a store clone.
    events: architect::PubSub<NotifyEvent>,
}

impl Store {
    /// Construct from an already-opened connection. The caller must
    /// run [`crate::Migrator`] before invoking any method.
    #[must_use]
    pub fn new(conn: DatabaseConnection) -> Self {
        Self {
            conn,
            events: architect::PubSub::sliding(256),
        }
    }

    #[must_use]
    pub fn conn(&self) -> &DatabaseConnection {
        &self.conn
    }

    /// Publish a change to every `events` subscriber. Call only after
    /// the write succeeded — subscribers fold these into state fetched
    /// via `list()`, so a phantom event would desync them.
    fn publish(&self, event: NotifyEvent) {
        self.events.publish(event);
    }

    /// Mint a row from a rule's [`NewNotification`] — id + timestamps
    /// assigned here so every delivery channel sees one identity.
    #[must_use]
    pub fn mint(new: NewNotification) -> Notification {
        Notification {
            id: Uuid::new_v4(),
            kind: new.kind,
            title: new.title,
            body: new.body,
            source: new.source,
            actor: new.actor,
            created_at: Utc::now(),
            read_at: None,
        }
    }

    /// Persist an already-minted notification exactly as given and
    /// publish it — the in-app delivery path ([`crate::InApp`] wraps
    /// exactly this).
    pub async fn insert(&self, row: Notification) -> Result<(), NotifyError> {
        entity::from_proto(&row)
            .into_active_model()
            // `into_active_model` marks fields Unchanged; INSERT only
            // writes Set values, so promote them all.
            .reset_all()
            .insert(&self.conn)
            .await
            .map_err(db_err)?;
        self.publish(NotifyEvent::Upserted(row));
        Ok(())
    }

    /// [`Self::mint`] + [`Self::insert`] in one call — the convenience
    /// for tests and single-channel callers.
    pub async fn push(&self, new: NewNotification) -> Result<Notification, NotifyError> {
        let row = Self::mint(new);
        self.insert(row.clone()).await?;
        Ok(row)
    }
}

fn db_err(e: sea_orm::DbErr) -> NotifyError {
    NotifyError::Backend(e.to_string())
}

impl notify_proto::Notify for Store {
    async fn list(&self, filter: NotifyListFilter) -> Result<Vec<Notification>, NotifyError> {
        let mut q = Notifications::find()
            .order_by_desc(Column::CreatedAt)
            // Tie-break equal timestamps so pages don't shear.
            .order_by_desc(Column::Id);
        if filter.unread_only {
            q = q.filter(Column::ReadAt.is_null());
        }
        let rows = q
            .offset(u64::from(filter.offset.unwrap_or(0)))
            .limit(filter.limit.map_or(DEFAULT_PAGE, u64::from))
            .all(&self.conn)
            .await
            .map_err(db_err)?;
        Ok(rows.into_iter().map(entity::Model::into_proto).collect())
    }

    async fn mark_read(&self, id: Uuid) -> Result<Notification, NotifyError> {
        let row = Notifications::find_by_id(id)
            .one(&self.conn)
            .await
            .map_err(db_err)?
            .ok_or_else(|| NotifyError::NotFound(id.to_string()))?;
        if row.read_at.is_some() {
            // Idempotent: keep the original read timestamp, publish
            // nothing (no state changed).
            return Ok(row.into_proto());
        }
        let mut active = row.into_active_model();
        active.read_at = Set(Some(Utc::now()));
        let updated = active.update(&self.conn).await.map_err(db_err)?;
        let out = updated.into_proto();
        self.publish(NotifyEvent::Upserted(out.clone()));
        Ok(out)
    }

    async fn mark_all_read(&self) -> Result<u64, NotifyError> {
        // Row-by-row so each flip publishes (the fold contract). The
        // unread set is bell-sized, not unbounded — reads bound it.
        let unread = Notifications::find()
            .filter(Column::ReadAt.is_null())
            .all(&self.conn)
            .await
            .map_err(db_err)?;
        let now = Utc::now();
        let mut flipped = 0u64;
        for row in unread {
            let mut active = row.into_active_model();
            active.read_at = Set(Some(now));
            let updated = active.update(&self.conn).await.map_err(db_err)?;
            self.publish(NotifyEvent::Upserted(updated.into_proto()));
            flipped += 1;
        }
        Ok(flipped)
    }

    async fn delete(&self, id: Uuid) -> Result<(), NotifyError> {
        let res = Notifications::delete_by_id(id)
            .exec(&self.conn)
            .await
            .map_err(db_err)?;
        if res.rows_affected > 0 {
            self.publish(NotifyEvent::Deleted(id));
        }
        Ok(())
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream host
/// the hub it attaches subscriber sinks to. Publishing happens in the
/// `Notify` impl above (and [`Store::push`]), on every successful
/// mutation.
impl notify_proto::NotifyStreamSource for Store {
    fn events_hub(&self) -> &architect::PubSub<NotifyEvent> {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use notify_proto::Notify as _;

    use super::*;

    async fn mem_store() -> Store {
        let conn = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("connect");
        use sea_orm_migration::MigratorTrait as _;
        crate::Migrator::up(&conn, None).await.expect("migrate");
        Store::new(conn)
    }

    fn draft(title: &str) -> NewNotification {
        NewNotification {
            kind: NotifyKind::TaskCompleted,
            title: title.into(),
            body: String::new(),
            source: NotifySource {
                service: "task".into(),
                entity: "e".into(),
                href: "/tasks".into(),
            },
            actor: String::new(),
        }
    }

    #[tokio::test]
    async fn push_list_mark_read_round_trip() {
        let store = mem_store().await;
        let a = store.push(draft("first")).await.expect("push a");
        let b = store.push(draft("second")).await.expect("push b");

        // Newest first; both unread.
        let all = store.list(NotifyListFilter::default()).await.expect("list");
        assert_eq!(
            all.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![b.id, a.id]
        );
        assert!(all.iter().all(|n| n.read_at.is_none()));

        // mark_read flips exactly one; unread_only drops it.
        let read = store.mark_read(a.id).await.expect("mark read");
        assert!(read.read_at.is_some());
        let again = store.mark_read(a.id).await.expect("idempotent");
        assert_eq!(again.read_at, read.read_at, "second mark keeps the stamp");
        let unread = store
            .list(NotifyListFilter {
                unread_only: true,
                ..Default::default()
            })
            .await
            .expect("unread");
        assert_eq!(unread.iter().map(|n| n.id).collect::<Vec<_>>(), vec![b.id]);

        // mark_all_read flips the rest; delete is idempotent.
        assert_eq!(store.mark_all_read().await.expect("mark all"), 1);
        assert_eq!(store.mark_all_read().await.expect("none left"), 0);
        store.delete(b.id).await.expect("delete");
        store.delete(b.id).await.expect("delete again");
        let all = store.list(NotifyListFilter::default()).await.expect("list");
        assert_eq!(all.len(), 1);
    }

    // The stream path (publish → `#[subscribe]` fan-out → client
    // fold) needs a bound vox transport, so it is covered end-to-end
    // in `task-server`'s `notify_e2e` test over the in-process
    // `LocalServer` — an unbound `vox::channel` Tx attached straight
    // to the hub never drains (no sink to resolve).

    #[tokio::test]
    async fn list_windows_are_stable() {
        let store = mem_store().await;
        for i in 0..5 {
            store.push(draft(&format!("n{i}"))).await.expect("push");
        }
        let page1 = store
            .list(NotifyListFilter {
                limit: Some(2),
                ..Default::default()
            })
            .await
            .expect("p1");
        let page2 = store
            .list(NotifyListFilter {
                limit: Some(2),
                offset: Some(2),
                ..Default::default()
            })
            .await
            .expect("p2");
        assert_eq!(page1.len(), 2);
        assert_eq!(page2.len(), 2);
        let all = store.list(NotifyListFilter::default()).await.expect("all");
        let tiled: Vec<_> = page1.iter().chain(&page2).map(|n| n.id).collect();
        assert_eq!(tiled, all.iter().take(4).map(|n| n.id).collect::<Vec<_>>());
    }
}
