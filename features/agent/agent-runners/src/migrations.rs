//! SeaORM migrator for the runner registry.
//!
//! One table. `id` is the primary key so a re-registering runner
//! upserts rather than duplicating; `kind` is a column because
//! `backends_by_kind` filters on it; everything else rides in
//! `json`. See the crate docs for why.

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260810_000001_create_agent_backends::Migration),
            Box::new(m20260810_000002_create_agent_runs::Migration),
            Box::new(m20260810_000003_create_agent_questions::Migration),
        ]
    }
}

mod m20260810_000001_create_agent_backends {
    use sea_orm_migration::prelude::*;

    pub struct Migration;

    impl MigrationName for Migration {
        fn name(&self) -> &str {
            "m20260810_000001_create_agent_backends"
        }
    }

    #[derive(DeriveIden)]
    enum AgentBackends {
        Table,
        Id,
        Kind,
        Json,
    }

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .create_table(
                    Table::create()
                        .table(AgentBackends::Table)
                        .if_not_exists()
                        .col(
                            ColumnDef::new(AgentBackends::Id)
                                .string()
                                .not_null()
                                .primary_key(),
                        )
                        .col(ColumnDef::new(AgentBackends::Kind).string().not_null())
                        .col(ColumnDef::new(AgentBackends::Json).text().not_null())
                        .to_owned(),
                )
                .await?;

            manager
                .create_index(
                    Index::create()
                        .if_not_exists()
                        .name("idx_agent_backends_kind")
                        .table(AgentBackends::Table)
                        .col(AgentBackends::Kind)
                        .to_owned(),
                )
                .await
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .drop_table(
                    Table::drop()
                        .table(AgentBackends::Table)
                        .if_exists()
                        .to_owned(),
                )
                .await
        }
    }
}

mod m20260810_000002_create_agent_runs {
    use sea_orm_migration::prelude::*;

    pub struct Migration;

    impl MigrationName for Migration {
        fn name(&self) -> &str {
            "m20260810_000002_create_agent_runs"
        }
    }

    #[derive(DeriveIden)]
    enum AgentRuns {
        Table,
        Id,
        Ticket,
        Runner,
        Parent,
        Branch,
        WorktreePath,
        SessionPath,
        Status,
        ExitCode,
        StartedAt,
        HeartbeatAt,
        FinishedAt,
    }

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .create_table(
                    Table::create()
                        .table(AgentRuns::Table)
                        .if_not_exists()
                        .col(
                            ColumnDef::new(AgentRuns::Id)
                                .string()
                                .not_null()
                                .primary_key(),
                        )
                        .col(ColumnDef::new(AgentRuns::Ticket).string().not_null())
                        .col(ColumnDef::new(AgentRuns::Runner).string().not_null())
                        .col(ColumnDef::new(AgentRuns::Parent).string().not_null())
                        .col(ColumnDef::new(AgentRuns::Branch).string().not_null())
                        .col(ColumnDef::new(AgentRuns::WorktreePath).text().not_null())
                        .col(ColumnDef::new(AgentRuns::SessionPath).text().not_null())
                        .col(ColumnDef::new(AgentRuns::Status).string().not_null())
                        .col(ColumnDef::new(AgentRuns::ExitCode).string().not_null())
                        .col(ColumnDef::new(AgentRuns::StartedAt).string().not_null())
                        .col(ColumnDef::new(AgentRuns::HeartbeatAt).string().not_null())
                        .col(ColumnDef::new(AgentRuns::FinishedAt).string().not_null())
                        .to_owned(),
                )
                .await?;

            // The hot reads: a ticket's attempt history, what a
            // runner is doing, and the stale sweep.
            for (name, col) in [
                ("idx_agent_runs_ticket", AgentRuns::Ticket),
                ("idx_agent_runs_runner", AgentRuns::Runner),
                ("idx_agent_runs_status", AgentRuns::Status),
            ] {
                manager
                    .create_index(
                        Index::create()
                            .if_not_exists()
                            .name(name)
                            .table(AgentRuns::Table)
                            .col(col)
                            .to_owned(),
                    )
                    .await?;
            }
            Ok(())
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .drop_table(Table::drop().table(AgentRuns::Table).if_exists().to_owned())
                .await
        }
    }
}

mod m20260810_000003_create_agent_questions {
    use sea_orm_migration::prelude::*;

    pub struct Migration;

    impl MigrationName for Migration {
        fn name(&self) -> &str {
            "m20260810_000003_create_agent_questions"
        }
    }

    #[derive(DeriveIden)]
    enum AgentQuestions {
        Table,
        Id,
        Ticket,
        Resolved,
        Json,
    }

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .create_table(
                    Table::create()
                        .table(AgentQuestions::Table)
                        .if_not_exists()
                        .col(
                            ColumnDef::new(AgentQuestions::Id)
                                .string()
                                .not_null()
                                .primary_key(),
                        )
                        .col(ColumnDef::new(AgentQuestions::Ticket).string().not_null())
                        .col(
                            ColumnDef::new(AgentQuestions::Resolved)
                                .integer()
                                .not_null(),
                        )
                        .col(ColumnDef::new(AgentQuestions::Json).text().not_null())
                        .to_owned(),
                )
                .await?;

            // The grill queue reads unresolved-by-ticket; both
            // columns are in every hot query.
            for (name, col) in [
                ("idx_agent_questions_ticket", AgentQuestions::Ticket),
                ("idx_agent_questions_resolved", AgentQuestions::Resolved),
            ] {
                manager
                    .create_index(
                        Index::create()
                            .if_not_exists()
                            .name(name)
                            .table(AgentQuestions::Table)
                            .col(col)
                            .to_owned(),
                    )
                    .await?;
            }
            Ok(())
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .drop_table(
                    Table::drop()
                        .table(AgentQuestions::Table)
                        .if_exists()
                        .to_owned(),
                )
                .await
        }
    }
}
