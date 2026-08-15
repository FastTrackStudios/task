//! SeaORM migrator for the notifications schema.

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260727_000001_create_notifications::Migration)]
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
