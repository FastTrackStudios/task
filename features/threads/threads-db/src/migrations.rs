//! SeaORM migrator for the threads schema (two tables + anchor/thread
//! indexes). Schema mirrors `threads-proto`'s `Thread` + `Message`.

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260609_000001_create_threads::Migration)]
    }
}

mod m20260609_000001_create_threads {
    use sea_orm_migration::prelude::*;

    #[derive(DeriveMigrationName)]
    pub struct Migration;

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .create_table(
                    Table::create()
                        .table(ThreadsThreads::Table)
                        .if_not_exists()
                        .col(
                            ColumnDef::new(ThreadsThreads::Id)
                                .uuid()
                                .not_null()
                                .primary_key(),
                        )
                        .col(ColumnDef::new(ThreadsThreads::OrgId).uuid().not_null())
                        .col(
                            ColumnDef::new(ThreadsThreads::EntityType)
                                .string()
                                .not_null(),
                        )
                        .col(ColumnDef::new(ThreadsThreads::EntityId).uuid().not_null())
                        .col(ColumnDef::new(ThreadsThreads::Title).text().not_null())
                        .col(ColumnDef::new(ThreadsThreads::Kind).string().not_null())
                        .col(
                            ColumnDef::new(ThreadsThreads::Resolved)
                                .boolean()
                                .not_null(),
                        )
                        .col(ColumnDef::new(ThreadsThreads::ResolvedBy).uuid().null())
                        .col(
                            ColumnDef::new(ThreadsThreads::SourceKind)
                                .string()
                                .not_null(),
                        )
                        .col(ColumnDef::new(ThreadsThreads::SourceRef).string().null())
                        .col(ColumnDef::new(ThreadsThreads::SourceUrl).string().null())
                        .col(ColumnDef::new(ThreadsThreads::CreatedBy).uuid().not_null())
                        .col(
                            ColumnDef::new(ThreadsThreads::CreatedAt)
                                .timestamp_with_time_zone()
                                .not_null(),
                        )
                        .col(
                            ColumnDef::new(ThreadsThreads::UpdatedAt)
                                .timestamp_with_time_zone()
                                .not_null(),
                        )
                        .to_owned(),
                )
                .await?;

            manager
                .create_table(
                    Table::create()
                        .table(ThreadsMessages::Table)
                        .if_not_exists()
                        .col(
                            ColumnDef::new(ThreadsMessages::Id)
                                .uuid()
                                .not_null()
                                .primary_key(),
                        )
                        .col(ColumnDef::new(ThreadsMessages::ThreadId).uuid().not_null())
                        .col(ColumnDef::new(ThreadsMessages::OrgId).uuid().not_null())
                        .col(ColumnDef::new(ThreadsMessages::AuthorId).uuid().null())
                        .col(
                            ColumnDef::new(ThreadsMessages::AuthorLabel)
                                .string()
                                .not_null(),
                        )
                        .col(ColumnDef::new(ThreadsMessages::Body).text().not_null())
                        .col(ColumnDef::new(ThreadsMessages::ReplyTo).uuid().null())
                        .col(
                            ColumnDef::new(ThreadsMessages::SourceKind)
                                .string()
                                .not_null(),
                        )
                        .col(ColumnDef::new(ThreadsMessages::ExternalId).string().null())
                        .col(ColumnDef::new(ThreadsMessages::OriginalText).text().null())
                        .col(ColumnDef::new(ThreadsMessages::SourceUrl).string().null())
                        .col(
                            ColumnDef::new(ThreadsMessages::PostedAt)
                                .timestamp_with_time_zone()
                                .not_null(),
                        )
                        .col(
                            ColumnDef::new(ThreadsMessages::CreatedAt)
                                .timestamp_with_time_zone()
                                .not_null(),
                        )
                        .col(
                            ColumnDef::new(ThreadsMessages::UpdatedAt)
                                .timestamp_with_time_zone()
                                .not_null(),
                        )
                        .to_owned(),
                )
                .await?;

            // Anchored-read indexes.
            manager
                .create_index(
                    Index::create()
                        .if_not_exists()
                        .name("idx_threads_threads_anchor")
                        .table(ThreadsThreads::Table)
                        .col(ThreadsThreads::EntityType)
                        .col(ThreadsThreads::EntityId)
                        .to_owned(),
                )
                .await?;
            manager
                .create_index(
                    Index::create()
                        .if_not_exists()
                        .name("idx_threads_messages_thread")
                        .table(ThreadsMessages::Table)
                        .col(ThreadsMessages::ThreadId)
                        .to_owned(),
                )
                .await
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .drop_table(
                    Table::drop()
                        .table(ThreadsMessages::Table)
                        .if_exists()
                        .to_owned(),
                )
                .await?;
            manager
                .drop_table(
                    Table::drop()
                        .table(ThreadsThreads::Table)
                        .if_exists()
                        .to_owned(),
                )
                .await
        }
    }

    #[derive(Iden)]
    enum ThreadsThreads {
        Table,
        Id,
        OrgId,
        EntityType,
        EntityId,
        Title,
        Kind,
        Resolved,
        ResolvedBy,
        SourceKind,
        SourceRef,
        SourceUrl,
        CreatedBy,
        CreatedAt,
        UpdatedAt,
    }

    #[derive(Iden)]
    enum ThreadsMessages {
        Table,
        Id,
        ThreadId,
        OrgId,
        AuthorId,
        AuthorLabel,
        Body,
        ReplyTo,
        SourceKind,
        ExternalId,
        OriginalText,
        SourceUrl,
        PostedAt,
        CreatedAt,
        UpdatedAt,
    }
}
