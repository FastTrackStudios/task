//! [`threads_proto::ThreadsService`] impl over SeaORM.

use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set,
};
use threads_proto::service::{CreateThreadRequest, PostMessageRequest, ThreadsService};
use threads_proto::{Message, Thread, ThreadsError};
use uuid::Uuid;

use crate::entity::{
    MessageActive, MessageColumn, MessageEntity, MessageModel, ThreadActive, ThreadColumn,
    ThreadEntity, ThreadModel,
};

/// SeaORM-backed threads store. Cheap to `Clone` (the connection is
/// `Arc`-backed). The caller runs [`crate::Migrator`] before use.
#[derive(Clone)]
pub struct Store {
    conn: DatabaseConnection,
}

impl Store {
    #[must_use]
    pub fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }

    #[must_use]
    pub fn conn(&self) -> &DatabaseConnection {
        &self.conn
    }
}

/// Map a `sea_orm::DbErr` to the trait-boundary error.
fn db(e: sea_orm::DbErr) -> ThreadsError {
    ThreadsError::Backend(e.to_string())
}

fn thread_from(m: ThreadModel) -> Thread {
    Thread {
        id: m.id,
        org_id: m.org_id,
        entity_type: m.entity_type,
        entity_id: m.entity_id,
        anchor_path: m.anchor_path,
        title: m.title,
        kind: m.kind,
        resolved: m.resolved,
        resolved_by: m.resolved_by,
        source_kind: m.source_kind,
        source_ref: m.source_ref,
        source_url: m.source_url,
        created_by: m.created_by,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

fn message_from(m: MessageModel) -> Message {
    Message {
        id: m.id,
        thread_id: m.thread_id,
        org_id: m.org_id,
        author_id: m.author_id,
        author_label: m.author_label,
        body: m.body,
        reply_to: m.reply_to,
        source_kind: m.source_kind,
        external_id: m.external_id,
        original_text: m.original_text,
        source_url: m.source_url,
        posted_at: m.posted_at,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

impl ThreadsService for Store {
    async fn list_threads(
        &self,
        entity_type: String,
        entity_id: Uuid,
    ) -> Result<Vec<Thread>, ThreadsError> {
        let rows = ThreadEntity::find()
            .filter(ThreadColumn::EntityType.eq(entity_type))
            .filter(ThreadColumn::EntityId.eq(entity_id))
            .order_by_desc(ThreadColumn::UpdatedAt)
            .all(&self.conn)
            .await
            .map_err(db)?;
        Ok(rows.into_iter().map(thread_from).collect())
    }

    async fn list_threads_at(
        &self,
        entity_type: String,
        entity_id: Uuid,
        anchor_path: String,
    ) -> Result<Vec<Thread>, ThreadsError> {
        // One entity's threads are few: filter the place here rather than
        // with a LIKE whose wildcards the path would have to escape.
        let all = self.list_threads(entity_type, entity_id).await?;
        Ok(all
            .into_iter()
            .filter(|t| {
                threads_proto::thread::anchor_within(t.anchor_path.as_deref(), &anchor_path)
            })
            .collect())
    }

    async fn get_thread(&self, id: Uuid) -> Result<Thread, ThreadsError> {
        ThreadEntity::find_by_id(id)
            .one(&self.conn)
            .await
            .map_err(db)?
            .map(thread_from)
            .ok_or_else(|| ThreadsError::ThreadNotFound(id.to_string()))
    }

    async fn create_thread(&self, req: CreateThreadRequest) -> Result<Thread, ThreadsError> {
        if req.title.trim().is_empty() {
            return Err(ThreadsError::Invalid("title is required".into()));
        }
        let now = Utc::now();
        let kind = if req.kind.trim().is_empty() {
            "discussion".to_string()
        } else {
            req.kind
        };
        let source_kind = if req.source_kind.trim().is_empty() {
            "native".to_string()
        } else {
            req.source_kind
        };
        let am = ThreadActive {
            id: Set(Uuid::new_v4()),
            org_id: Set(req.org_id),
            entity_type: Set(req.entity_type),
            entity_id: Set(req.entity_id),
            anchor_path: Set(threads_proto::thread::anchor_path(
                req.anchor_path.as_deref(),
            )),
            title: Set(req.title),
            kind: Set(kind),
            resolved: Set(false),
            resolved_by: Set(None),
            source_kind: Set(source_kind),
            source_ref: Set(req.source_ref),
            source_url: Set(req.source_url),
            created_by: Set(req.created_by),
            created_at: Set(now),
            updated_at: Set(now),
        };
        let m = am.insert(&self.conn).await.map_err(db)?;
        Ok(thread_from(m))
    }

    async fn list_messages(&self, thread_id: Uuid) -> Result<Vec<Message>, ThreadsError> {
        let rows = MessageEntity::find()
            .filter(MessageColumn::ThreadId.eq(thread_id))
            .order_by_asc(MessageColumn::PostedAt)
            .all(&self.conn)
            .await
            .map_err(db)?;
        Ok(rows.into_iter().map(message_from).collect())
    }

    async fn post_message(&self, req: PostMessageRequest) -> Result<Message, ThreadsError> {
        let now = Utc::now();
        let source_kind = if req.source_kind.trim().is_empty() {
            "native".to_string()
        } else {
            req.source_kind
        };
        let am = MessageActive {
            id: Set(Uuid::new_v4()),
            thread_id: Set(req.thread_id),
            org_id: Set(req.org_id),
            author_id: Set(req.author_id),
            author_label: Set(req.author_label),
            body: Set(req.body),
            reply_to: Set(req.reply_to),
            source_kind: Set(source_kind),
            external_id: Set(req.external_id),
            original_text: Set(req.original_text),
            source_url: Set(req.source_url),
            posted_at: Set(req.posted_at.unwrap_or(now)),
            created_at: Set(now),
            updated_at: Set(now),
        };
        let m = am.insert(&self.conn).await.map_err(db)?;
        // Bump the parent thread so activity-ordered lists float it up.
        // Best-effort — a missing thread just means no rows updated.
        let _ = ThreadEntity::update_many()
            .col_expr(ThreadColumn::UpdatedAt, Expr::value(now))
            .filter(ThreadColumn::Id.eq(req.thread_id))
            .exec(&self.conn)
            .await;
        Ok(message_from(m))
    }

    async fn set_resolved(
        &self,
        thread_id: Uuid,
        resolved: bool,
        by: Option<Uuid>,
    ) -> Result<Thread, ThreadsError> {
        let existing = ThreadEntity::find_by_id(thread_id)
            .one(&self.conn)
            .await
            .map_err(db)?
            .ok_or_else(|| ThreadsError::ThreadNotFound(thread_id.to_string()))?;
        let mut am = existing.into_active_model();
        am.resolved = Set(resolved);
        am.resolved_by = Set(by);
        am.updated_at = Set(Utc::now());
        let m = am.update(&self.conn).await.map_err(db)?;
        Ok(thread_from(m))
    }

    async fn delete_thread(&self, id: Uuid) -> Result<(), ThreadsError> {
        // Code-level cascade (DB-portable): drop messages, then the thread.
        MessageEntity::delete_many()
            .filter(MessageColumn::ThreadId.eq(id))
            .exec(&self.conn)
            .await
            .map_err(db)?;
        ThreadEntity::delete_by_id(id)
            .exec(&self.conn)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn delete_message(&self, id: Uuid) -> Result<(), ThreadsError> {
        MessageEntity::delete_by_id(id)
            .exec(&self.conn)
            .await
            .map_err(db)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sea_orm_migration::MigratorTrait as _;

    use super::*;

    async fn store(dir: &std::path::Path) -> Store {
        let url = format!("sqlite://{}?mode=rwc", dir.join("threads.sqlite").display());
        let conn = sea_orm::Database::connect(url).await.unwrap();
        crate::Migrator::up(&conn, None).await.unwrap();
        Store::new(conn)
    }

    fn on(entity_id: Uuid, path: Option<&str>, title: &str) -> CreateThreadRequest {
        CreateThreadRequest {
            org_id: Uuid::nil(),
            entity_type: "doc".into(),
            entity_id,
            anchor_path: path.map(str::to_owned),
            title: title.into(),
            kind: String::new(),
            created_by: Uuid::nil(),
            source_kind: String::new(),
            source_ref: None,
            source_url: None,
        }
    }

    #[tokio::test]
    async fn a_thread_keeps_its_place_inside_the_entity() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path()).await;
        let doc = Uuid::new_v4();
        let whole = s.create_thread(on(doc, None, "Whole")).await.unwrap();
        let entry = s
            .create_thread(on(doc, Some("/entries/e1/"), "Entry"))
            .await
            .unwrap();
        let note = s
            .create_thread(on(doc, Some("entries/e1/notes"), "Note"))
            .await
            .unwrap();
        s.create_thread(on(doc, Some("entries/e10"), "Other"))
            .await
            .unwrap();
        s.create_thread(on(doc, Some(" "), "Blank is whole"))
            .await
            .unwrap();

        assert_eq!(whole.anchor_path, None);
        assert_eq!(entry.anchor_path.as_deref(), Some("entries/e1"), "trimmed");
        assert_eq!(
            s.get_thread(note.id).await.unwrap().anchor_path.as_deref(),
            Some("entries/e1/notes")
        );

        let all = s.list_threads("doc".into(), doc).await.unwrap();
        assert_eq!(all.len(), 5, "the entity lists every place");
        let mut at: Vec<String> = s
            .list_threads_at("doc".into(), doc, "entries/e1".into())
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        at.sort();
        assert_eq!(at, ["Entry", "Note"], "the place and under, not a sibling");
        let everything = s
            .list_threads_at("doc".into(), doc, String::new())
            .await
            .unwrap();
        assert_eq!(everything.len(), 5);
        assert!(
            s.list_threads_at("doc".into(), Uuid::new_v4(), "entries/e1".into())
                .await
                .unwrap()
                .is_empty()
        );
    }
}
