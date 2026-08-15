//! SeaORM migrator for the identity-locker schema.

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(
            m20260719_000001_create_linked_servers::Migration,
        )]
    }
}

mod m20260719_000001_create_linked_servers {
    use sea_orm_migration::prelude::*;

    #[derive(DeriveMigrationName)]
    pub struct Migration;

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .create_table(
                    Table::create()
                        .table(LinkedServers::Table)
                        .if_not_exists()
                        .col(
                            ColumnDef::new(LinkedServers::Id)
                                .uuid()
                                .not_null()
                                .primary_key(),
                        )
                        .col(
                            ColumnDef::new(LinkedServers::HomeUserId)
                                .uuid()
                                .not_null(),
                        )
                        .col(ColumnDef::new(LinkedServers::Label).string().not_null())
                        .col(
                            ColumnDef::new(LinkedServers::RemoteUrl)
                                .string()
                                .not_null(),
                        )
                        .col(
                            ColumnDef::new(LinkedServers::RemoteSlug)
                                .string()
                                .not_null(),
                        )
                        .col(ColumnDef::new(LinkedServers::RemoteUserId).uuid().null())
                        .col(ColumnDef::new(LinkedServers::RemoteEmail).string().null())
                        .col(
                            ColumnDef::new(LinkedServers::TokenCiphertext)
                                .string()
                                .not_null(),
                        )
                        .col(ColumnDef::new(LinkedServers::ExpiresAt).big_integer().null())
                        .col(
                            ColumnDef::new(LinkedServers::CreatedAt)
                                .big_integer()
                                .not_null(),
                        )
                        .col(
                            ColumnDef::new(LinkedServers::UpdatedAt)
                                .big_integer()
                                .not_null(),
                        )
                        .to_owned(),
                )
                .await?;

            manager
                .create_index(
                    Index::create()
                        .name("idx_linked_servers_home_url_slug")
                        .table(LinkedServers::Table)
                        .col(LinkedServers::HomeUserId)
                        .col(LinkedServers::RemoteUrl)
                        .col(LinkedServers::RemoteSlug)
                        .unique()
                        .to_owned(),
                )
                .await
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .drop_table(
                    Table::drop()
                        .table(LinkedServers::Table)
                        .if_exists()
                        .to_owned(),
                )
                .await
        }
    }

    // ── Iden ──────────────────────────────────────────────────────────

    #[derive(Iden)]
    enum LinkedServers {
        Table,
        Id,
        HomeUserId,
        Label,
        RemoteUrl,
        RemoteSlug,
        RemoteUserId,
        RemoteEmail,
        TokenCiphertext,
        ExpiresAt,
        CreatedAt,
        UpdatedAt,
    }
}
