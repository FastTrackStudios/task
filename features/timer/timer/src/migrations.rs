//! SeaORM migrator for the timer schema.

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260524_000001_create_timer::Migration),
            Box::new(m20260601_000002_add_invoice_id::Migration),
        ]
    }
}

/// Adds the nullable `invoice_id` link to `timer_work_sessions` so the
/// finance backend can mark which sessions have been billed.
mod m20260601_000002_add_invoice_id {
    use sea_orm_migration::prelude::*;

    pub struct Migration;

    // Manual (not derived): the derive keys off the struct name, which
    // is `Migration` for every nested module here — two of them would
    // collide on `seaql_migrations.version`. Give this one an explicit
    // unique version.
    impl MigrationName for Migration {
        fn name(&self) -> &'static str {
            "m20260601_000002_add_invoice_id"
        }
    }

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("timer_work_sessions"))
                        .add_column(ColumnDef::new(Alias::new("invoice_id")).uuid().null())
                        .to_owned(),
                )
                .await
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("timer_work_sessions"))
                        .drop_column(Alias::new("invoice_id"))
                        .to_owned(),
                )
                .await
        }
    }
}

mod m20260524_000001_create_timer {
    use sea_orm_migration::prelude::*;

    #[derive(DeriveMigrationName)]
    pub struct Migration;

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            create_tables(manager).await?;
            create_indexes(manager).await
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            for table in [
                TimerWorkSessionTags::Table.into_iden(),
                TimerWorkSessions::Table.into_iden(),
                TimerTags::Table.into_iden(),
                TimerOrgMemberRates::Table.into_iden(),
                TimerProjectMemberRates::Table.into_iden(),
                TimerClients::Table.into_iden(),
            ] {
                manager
                    .drop_table(Table::drop().table(table).if_exists().to_owned())
                    .await?;
            }
            Ok(())
        }
    }

    async fn create_tables(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(TimerClients::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(TimerClients::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(TimerClients::OrgId).uuid().not_null())
                    .col(ColumnDef::new(TimerClients::Name).string().not_null())
                    .col(ColumnDef::new(TimerClients::Notes).text().not_null())
                    .col(ColumnDef::new(TimerClients::Archived).boolean().not_null())
                    .col(
                        ColumnDef::new(TimerClients::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerClients::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(TimerWorkSessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(TimerWorkSessions::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(TimerWorkSessions::OrgId).uuid().not_null())
                    .col(ColumnDef::new(TimerWorkSessions::UserId).uuid().not_null())
                    .col(ColumnDef::new(TimerWorkSessions::ProjectId).uuid().null())
                    .col(
                        ColumnDef::new(TimerWorkSessions::ProjectPath)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerWorkSessions::Description)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerWorkSessions::StartTime)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerWorkSessions::EndTime)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(TimerWorkSessions::Billable)
                            .boolean()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerWorkSessions::RateCents)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerWorkSessions::Currency)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerWorkSessions::TaskNotePath)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerWorkSessions::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerWorkSessions::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(TimerTags::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(TimerTags::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(TimerTags::OrgId).uuid().not_null())
                    .col(ColumnDef::new(TimerTags::Name).string().not_null())
                    .col(ColumnDef::new(TimerTags::Color).string().not_null())
                    .col(
                        ColumnDef::new(TimerTags::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerTags::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(TimerWorkSessionTags::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(TimerWorkSessionTags::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(TimerWorkSessionTags::WorkSessionId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerWorkSessionTags::TagId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerWorkSessionTags::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                TimerWorkSessionTags::Table,
                                TimerWorkSessionTags::WorkSessionId,
                            )
                            .to(TimerWorkSessions::Table, TimerWorkSessions::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(TimerWorkSessionTags::Table, TimerWorkSessionTags::TagId)
                            .to(TimerTags::Table, TimerTags::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(TimerProjectMemberRates::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(TimerProjectMemberRates::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(TimerProjectMemberRates::ProjectId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerProjectMemberRates::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerProjectMemberRates::HourlyCents)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerProjectMemberRates::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerProjectMemberRates::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(TimerOrgMemberRates::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(TimerOrgMemberRates::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(TimerOrgMemberRates::OrgId).uuid().not_null())
                    .col(
                        ColumnDef::new(TimerOrgMemberRates::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerOrgMemberRates::HourlyCents)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerOrgMemberRates::Currency)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerOrgMemberRates::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TimerOrgMemberRates::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn create_indexes(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
        // The active-timer invariant. `UNIQUE (user_id)
        // WHERE end_time IS NULL` enforces "at most one open
        // session per user" at the DB layer — the store's
        // `start_timer` race-checks via `INSERT ... ON
        // CONFLICT DO NOTHING` against this index.
        manager
            .create_index(
                Index::create()
                    .name("ux_timer_work_sessions_user_active")
                    .table(TimerWorkSessions::Table)
                    .col(TimerWorkSessions::UserId)
                    .unique()
                    .and_where(Expr::col(TimerWorkSessions::EndTime).is_null())
                    .to_owned(),
            )
            .await?;
        // Common reporting access pattern: timesheet for one
        // user over a date range.
        manager
            .create_index(
                Index::create()
                    .name("idx_timer_work_sessions_user_start")
                    .table(TimerWorkSessions::Table)
                    .col(TimerWorkSessions::UserId)
                    .col(TimerWorkSessions::StartTime)
                    .to_owned(),
            )
            .await?;
        // Project rollup.
        manager
            .create_index(
                Index::create()
                    .name("idx_timer_work_sessions_project")
                    .table(TimerWorkSessions::Table)
                    .col(TimerWorkSessions::ProjectId)
                    .col(TimerWorkSessions::StartTime)
                    .to_owned(),
            )
            .await?;
        // Unique tag-link triple.
        manager
            .create_index(
                Index::create()
                    .name("ux_timer_work_session_tags_pair")
                    .table(TimerWorkSessionTags::Table)
                    .col(TimerWorkSessionTags::WorkSessionId)
                    .col(TimerWorkSessionTags::TagId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        // Unique rate rows.
        manager
            .create_index(
                Index::create()
                    .name("ux_timer_project_member_rates_pair")
                    .table(TimerProjectMemberRates::Table)
                    .col(TimerProjectMemberRates::ProjectId)
                    .col(TimerProjectMemberRates::UserId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_timer_org_member_rates_pair")
                    .table(TimerOrgMemberRates::Table)
                    .col(TimerOrgMemberRates::OrgId)
                    .col(TimerOrgMemberRates::UserId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    // ── Iden ──────────────────────────────────────────────────────────

    #[derive(Iden)]
    enum TimerClients {
        Table,
        Id,
        OrgId,
        Name,
        Notes,
        Archived,
        CreatedAt,
        UpdatedAt,
    }

    #[derive(Iden)]
    enum TimerWorkSessions {
        Table,
        Id,
        OrgId,
        UserId,
        ProjectId,
        ProjectPath,
        Description,
        StartTime,
        EndTime,
        Billable,
        RateCents,
        Currency,
        TaskNotePath,
        CreatedAt,
        UpdatedAt,
    }

    #[derive(Iden)]
    enum TimerTags {
        Table,
        Id,
        OrgId,
        Name,
        Color,
        CreatedAt,
        UpdatedAt,
    }

    #[derive(Iden)]
    enum TimerWorkSessionTags {
        Table,
        Id,
        WorkSessionId,
        TagId,
        CreatedAt,
    }

    #[derive(Iden)]
    enum TimerProjectMemberRates {
        Table,
        Id,
        ProjectId,
        UserId,
        HourlyCents,
        CreatedAt,
        UpdatedAt,
    }

    #[derive(Iden)]
    enum TimerOrgMemberRates {
        Table,
        Id,
        OrgId,
        UserId,
        HourlyCents,
        Currency,
        CreatedAt,
        UpdatedAt,
    }
}
