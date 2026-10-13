//! SeaORM migrator for the notifications schema.

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260727_000001_create_notifications::Migration),
            Box::new(m20261012_000001_create_activity::Migration),
        ]
    }
}

/// The activity feed (`notify_proto::activity`) — history beside the
/// queue, in the same per-org `notify.sqlite`.
mod m20261012_000001_create_activity {
    use sea_orm_migration::prelude::*;

    pub struct Migration;

    // Manual name: `DeriveMigrationName` stamps the *file* stem, which
    // the create migration above already holds ("migrations").
    impl MigrationName for Migration {
        fn name(&self) -> &str {
            "m20261012_000001_create_activity"
        }
    }

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            let s = |c: Activity| ColumnDef::new(c).string().not_null().to_owned();
            manager
                .create_table(
                    Table::create()
                        .table(Activity::Table)
                        .if_not_exists()
                        .col(ColumnDef::new(Activity::Id).uuid().not_null().primary_key())
                        .col(
                            ColumnDef::new(Activity::At)
                                .timestamp_with_time_zone()
                                .not_null(),
                        )
                        .col(s(Activity::ActorId))
                        .col(s(Activity::ActorName))
                        .col(s(Activity::Kind))
                        .col(s(Activity::EntityType))
                        .col(s(Activity::EntityId))
                        .col(s(Activity::EntityTitle))
                        .col(s(Activity::Href))
                        .col(ColumnDef::new(Activity::ProjectId).uuid().null())
                        .col(s(Activity::Field))
                        .col(ColumnDef::new(Activity::FromValue).text().not_null())
                        .col(ColumnDef::new(Activity::ToValue).text().not_null())
                        .col(ColumnDef::new(Activity::Body).text().not_null())
                        .to_owned(),
                )
                .await?;
            // The three access patterns: the whole feed newest-first,
            // one entity's history, one project's.
            for (name, cols) in [
                ("idx_notify_activity_at", vec![Activity::At]),
                (
                    "idx_notify_activity_entity",
                    vec![Activity::EntityId, Activity::At],
                ),
                (
                    "idx_notify_activity_project",
                    vec![Activity::ProjectId, Activity::At],
                ),
            ] {
                let mut idx = Index::create();
                idx.name(name).table(Activity::Table);
                for c in cols {
                    idx.col(c);
                }
                manager.create_index(idx.to_owned()).await?;
            }
            Ok(())
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .drop_table(Table::drop().table(Activity::Table).if_exists().to_owned())
                .await
        }
    }

    #[derive(Iden, Clone, Copy)]
    enum Activity {
        #[iden = "notify_activity"]
        Table,
        Id,
        At,
        ActorId,
        ActorName,
        Kind,
        EntityType,
        EntityId,
        EntityTitle,
        Href,
        ProjectId,
        Field,
        FromValue,
        ToValue,
        Body,
    }
}

mod m20260727_000001_create_notifications {
    use sea_orm_migration::prelude::*;

    #[derive(DeriveMigrationName)]
    pub struct Migration;

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .create_table(
                    Table::create()
                        .table(Notifications::Table)
                        .if_not_exists()
                        .col(
                            ColumnDef::new(Notifications::Id)
                                .uuid()
                                .not_null()
                                .primary_key(),
                        )
                        .col(ColumnDef::new(Notifications::Kind).string().not_null())
                        .col(ColumnDef::new(Notifications::Title).string().not_null())
                        .col(ColumnDef::new(Notifications::Body).text().not_null())
                        .col(
                            ColumnDef::new(Notifications::SourceService)
                                .string()
                                .not_null(),
                        )
                        .col(
                            ColumnDef::new(Notifications::SourceEntity)
                                .string()
                                .not_null(),
                        )
                        .col(
                            ColumnDef::new(Notifications::SourceHref)
                                .string()
                                .not_null(),
                        )
                        .col(ColumnDef::new(Notifications::Actor).string().not_null())
                        .col(
                            ColumnDef::new(Notifications::CreatedAt)
                                .timestamp_with_time_zone()
                                .not_null(),
                        )
                        .col(
                            ColumnDef::new(Notifications::ReadAt)
                                .timestamp_with_time_zone()
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
            // The bell's two access patterns: newest-first pages, and
            // the unread scan (`read_at IS NULL` filtered then ordered
            // by recency — one composite covers both).
            manager
                .create_index(
                    Index::create()
                        .name("idx_notify_notifications_created")
                        .table(Notifications::Table)
                        .col(Notifications::CreatedAt)
                        .to_owned(),
                )
                .await?;
            manager
                .create_index(
                    Index::create()
                        .name("idx_notify_notifications_read_created")
                        .table(Notifications::Table)
                        .col(Notifications::ReadAt)
                        .col(Notifications::CreatedAt)
                        .to_owned(),
                )
                .await
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .drop_table(
                    Table::drop()
                        .table(Notifications::Table)
                        .if_exists()
                        .to_owned(),
                )
                .await
        }
    }

    #[derive(Iden)]
    enum Notifications {
        #[iden = "notify_notifications"]
        Table,
        Id,
        Kind,
        Title,
        Body,
        SourceService,
        SourceEntity,
        SourceHref,
        Actor,
        CreatedAt,
        ReadAt,
    }
}
