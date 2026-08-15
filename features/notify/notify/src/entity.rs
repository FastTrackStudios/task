//! SeaORM entity for the one notifications table.
//!
//! Hand-written (like `crdt-seaorm`'s) rather than
//! `#[derive(architect::Entity)]`-emitted: the wire struct nests
//! [`notify_proto::NotifySource`], which flattens to three columns
//! here. [`Model::into_proto`] / [`from_proto`] are the only
//! conversion points.

use notify_proto::{Notification, NotifyKind, NotifySource};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "notify_notifications")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// [`NotifyKind::as_str`] value; unknown strings (newer build's
    /// rows) decode as [`NotifyKind::Other`].
    pub kind: String,
    pub title: String,
    #[sea_orm(column_type = "Text")]
    pub body: String,
    pub source_service: String,
    pub source_entity: String,
    pub source_href: String,
    pub actor: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub read_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl Model {
    #[must_use]
    pub fn into_proto(self) -> Notification {
        Notification {
            id: self.id,
            kind: NotifyKind::parse(&self.kind),
            title: self.title,
            body: self.body,
            source: NotifySource {
                service: self.source_service,
                entity: self.source_entity,
                href: self.source_href,
            },
            actor: self.actor,
            created_at: self.created_at,
            read_at: self.read_at,
        }
    }
}

#[must_use]
pub fn from_proto(n: &Notification) -> Model {
    Model {
        id: n.id,
        kind: n.kind.as_str().to_owned(),
        title: n.title.clone(),
        body: n.body.clone(),
        source_service: n.source.service.clone(),
        source_entity: n.source.entity.clone(),
        source_href: n.source.href.clone(),
        actor: n.actor.clone(),
        created_at: n.created_at,
        read_at: n.read_at,
    }
}
