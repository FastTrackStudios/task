//! `ActivityFeed` impl over SeaORM — the per-org history table beside
//! the notifications queue, in the same `notify.sqlite`.
//!
//! Server-written only: the notifier and the comment hook call
//! [`ActivityStore::record`]; clients read through
//! [`notify_proto::ActivityFeed`]. Why here and not a crate of its own:
//! the queue and the history are written by the same rules from the
//! same hubs, live in the same per-org database, and are read by the
//! same surfaces (the bell links to `/activity`). A second crate would
//! be a second migrator, a second pool and a second snapshot entry for
//! one table.

use chrono::{DateTime, Utc};
use notify_proto::{Activity, ActivityEvent, ActivityFilter, ActivityKind, NotifyError};
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveModelTrait, Condition, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect};

/// Default page; also what a `limit: None` scroll-back gets.
pub const DEFAULT_PAGE: u32 = 50;
/// Hard cap on one page.
pub const MAX_PAGE: u32 = 500;

pub mod entity {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "notify_activity")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub at: chrono::DateTime<chrono::Utc>,
        pub actor_id: String,
        pub actor_name: String,
        pub kind: String,
        pub entity_type: String,
        pub entity_id: String,
        pub entity_title: String,
        pub href: String,
        pub project_id: Option<Uuid>,
        pub field: String,
        #[sea_orm(column_type = "Text")]
        pub from_value: String,
        #[sea_orm(column_type = "Text")]
        pub to_value: String,
        #[sea_orm(column_type = "Text")]
        pub body: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

use entity::{Column, Entity as Rows};

impl entity::Model {
    #[must_use]
    pub fn into_proto(self) -> Activity {
        Activity {
            id: self.id,
            at: self.at,
            actor_id: self.actor_id,
            actor_name: self.actor_name,
            kind: ActivityKind::parse(&self.kind),
            entity_type: self.entity_type,
            entity_id: self.entity_id,
            entity_title: self.entity_title,
            href: self.href,
            project_id: self.project_id,
            field: self.field,
            from: self.from_value,
            to: self.to_value,
            body: self.body,
        }
    }

    #[must_use]
    pub fn from_proto(a: &Activity) -> Self {
        Self {
            id: a.id,
            at: a.at,
            actor_id: a.actor_id.clone(),
            actor_name: a.actor_name.clone(),
            kind: a.kind.as_str().to_owned(),
            entity_type: a.entity_type.clone(),
            entity_id: a.entity_id.clone(),
            entity_title: a.entity_title.clone(),
            href: a.href.clone(),
            project_id: a.project_id,
            field: a.field.clone(),
            from_value: a.from.clone(),
            to_value: a.to.clone(),
            body: a.body.clone(),
        }
    }
}

/// What a rule produces — the store mints `id`; `at` is the rule's
/// (stamped where the change was seen, so a queued write keeps its
/// place in time).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewActivity {
    pub at: Option<DateTime<Utc>>,
    pub actor_id: String,
    pub actor_name: String,
    pub kind: Option<ActivityKind>,
    pub entity_type: String,
    pub entity_id: String,
    pub entity_title: String,
    pub href: String,
    pub project_id: Option<Uuid>,
    pub field: String,
    pub from: String,
    pub to: String,
    pub body: String,
}

impl NewActivity {
    /// A blank entry of `kind` about `(entity_type, entity_id)`.
    #[must_use]
    pub fn new(kind: ActivityKind, entity_type: &str, entity_id: impl ToString) -> Self {
        Self {
            at: Some(Utc::now()),
            kind: Some(kind),
            entity_type: entity_type.to_owned(),
            entity_id: entity_id.to_string(),
            ..Self::default()
        }
    }

    /// The row this becomes, with a fresh id.
    #[must_use]
    pub fn mint(self) -> Activity {
        Activity {
            id: Uuid::new_v4(),
            at: self.at.unwrap_or_else(Utc::now),
            actor_name: if self.actor_name.trim().is_empty() {
                "Someone".to_owned()
            } else {
                self.actor_name
            },
            actor_id: self.actor_id,
            kind: self.kind.unwrap_or(ActivityKind::Other),
            entity_type: self.entity_type,
            entity_id: self.entity_id,
            entity_title: self.entity_title,
            href: self.href,
            project_id: self.project_id,
            field: self.field,
            from: self.from,
            to: self.to,
            body: self.body,
        }
    }
}

#[derive(Clone)]
pub struct ActivityStore {
    conn: DatabaseConnection,
    /// Behind the `#[subscribe] fn events` stream. Sliding, like the
    /// queue's: a slow subscriber loses its oldest entries and recovers
    /// them with a `list` on resubscribe.
    events: architect::PubSub<ActivityEvent>,
}

fn db_err(e: sea_orm::DbErr) -> NotifyError {
    NotifyError::Backend(e.to_string())
}

impl ActivityStore {
    /// Over an already-migrated `notify.sqlite` connection (the same
    /// one [`crate::Store`] holds).
    #[must_use]
    pub fn new(conn: DatabaseConnection) -> Self {
        Self {
            conn,
            events: architect::PubSub::sliding(256),
        }
    }

    /// Mint + persist + publish.
    ///
    /// # Errors
    ///
    /// The database refused the write.
    pub async fn record(&self, new: NewActivity) -> Result<Activity, NotifyError> {
        let row = new.mint();
        self.insert(row.clone()).await?;
        Ok(row)
    }

    /// Persist an entry exactly as given and publish it.
    ///
    /// # Errors
    ///
    /// The database refused the write (a duplicate id among them).
    // t[impl activity.feed.live]
    pub async fn insert(&self, row: Activity) -> Result<(), NotifyError> {
        entity::Model::from_proto(&row)
            .into_active_model()
            .reset_all()
            .insert(&self.conn)
            .await
            .map_err(db_err)?;
        self.events.publish(ActivityEvent::Recorded(row));
        Ok(())
    }

    /// [`Self::insert`] unless an entry with that id is already there —
    /// the seed's idempotent write. Returns whether it wrote.
    ///
    /// # Errors
    ///
    /// The database refused the read or the write.
    pub async fn insert_if_absent(&self, row: Activity) -> Result<bool, NotifyError> {
        if Rows::find_by_id(row.id)
            .one(&self.conn)
            .await
            .map_err(db_err)?
            .is_some()
        {
            return Ok(false);
        }
        self.insert(row).await?;
        Ok(true)
    }
}

// t[impl activity.feed.order]
// t[impl activity.feed.filter]
impl notify_proto::ActivityFeed for ActivityStore {
    async fn list(&self, filter: ActivityFilter) -> Result<Vec<Activity>, NotifyError> {
        let mut cond = Condition::all();
        if let Some(t) = &filter.entity_type {
            cond = cond.add(Column::EntityType.eq(t.clone()));
        }
        if let Some(i) = &filter.entity_id {
            cond = cond.add(Column::EntityId.eq(i.clone()));
        }
        if let Some(p) = filter.project_id {
            cond = cond.add(Column::ProjectId.eq(p));
        }
        if !filter.kinds.is_empty() {
            cond = cond.add(Column::Kind.is_in(filter.kinds.iter().map(|k| k.as_str())));
        }
        if let Some(a) = &filter.actor_id {
            cond = cond.add(Column::ActorId.eq(a.clone()));
        }
        if let Some(c) = filter.before {
            // Strictly older in (at, id) order — the same order the
            // page is sorted in, so pages tile without overlap.
            cond = cond.add(
                Condition::any().add(Column::At.lt(c.at)).add(
                    Condition::all()
                        .add(Column::At.eq(c.at))
                        .add(Column::Id.lt(c.id)),
                ),
            );
        }
        let limit = filter.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
        let rows = Rows::find()
            .filter(cond)
            .order_by_desc(Column::At)
            .order_by_desc(Column::Id)
            .limit(u64::from(limit))
            .all(&self.conn)
            .await
            .map_err(db_err)?;
        Ok(rows.into_iter().map(entity::Model::into_proto).collect())
    }
}

impl notify_proto::ActivityFeedStreamSource for ActivityStore {
    fn events_hub(&self) -> &architect::PubSub<ActivityEvent> {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use notify_proto::{ActivityCursor, ActivityFeed as _};

    use super::*;

    async fn mem() -> ActivityStore {
        let conn = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("connect");
        use sea_orm_migration::MigratorTrait as _;
        crate::Migrator::up(&conn, None).await.expect("migrate");
        ActivityStore::new(conn)
    }

    fn entry(kind: ActivityKind, entity: &str, project: Option<Uuid>, secs: i64) -> NewActivity {
        let mut n = NewActivity::new(kind, "task", entity);
        n.at = Some(DateTime::from_timestamp(1_800_000_000 + secs, 0).expect("ts"));
        n.project_id = project;
        n
    }

    // t[verify activity.feed.order]
    // t[verify activity.feed.filter]
    #[tokio::test]
    async fn filters_and_the_before_cursor_tile_the_feed() {
        let store = mem().await;
        let p = Uuid::new_v4();
        for i in 0..6 {
            let entity = if i % 2 == 0 { "a" } else { "b" };
            store
                .record(entry(ActivityKind::StatusChanged, entity, Some(p), i))
                .await
                .expect("record");
        }
        store
            .record(entry(ActivityKind::Commented, "c", None, 10))
            .await
            .expect("record");

        let all = store.list(ActivityFilter::default()).await.expect("all");
        assert_eq!(all.len(), 7);
        assert!(all.windows(2).all(|w| w[0].at >= w[1].at), "newest first");
        assert_eq!(all[0].kind, ActivityKind::Commented);
        assert_eq!(all[0].actor_name, "Someone", "an unnamed actor is named");

        let a = store
            .list(ActivityFilter::entity("task", "a"))
            .await
            .expect("a");
        assert_eq!(a.len(), 3);
        let in_p = store
            .list(ActivityFilter::project(p))
            .await
            .expect("project");
        assert_eq!(in_p.len(), 6);
        let comments = store
            .list(ActivityFilter {
                kinds: vec![ActivityKind::Commented],
                ..Default::default()
            })
            .await
            .expect("kinds");
        assert_eq!(comments.len(), 1);

        // Pages of two, each continuing from the last one's oldest.
        let mut seen = Vec::new();
        let mut before = None;
        loop {
            let page = store
                .list(ActivityFilter {
                    before,
                    limit: Some(2),
                    ..Default::default()
                })
                .await
                .expect("page");
            let Some(last) = page.last() else { break };
            before = Some(ActivityCursor::after(last));
            seen.extend(page.iter().map(|a| a.id));
        }
        assert_eq!(seen, all.iter().map(|a| a.id).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn the_seed_write_is_idempotent() {
        let store = mem().await;
        let row = entry(ActivityKind::Created, "a", None, 0).mint();
        assert!(store.insert_if_absent(row.clone()).await.expect("first"));
        assert!(!store.insert_if_absent(row).await.expect("again"));
        assert_eq!(
            store
                .list(ActivityFilter::default())
                .await
                .expect("l")
                .len(),
            1
        );
    }
}
