//! SeaORM migrator for the agent-task queue schema.
//!
//! One migration creates the five tables — `agent_queues`,
//! `agent_tasks`, `agent_task_links`, `agent_task_comments`,
//! `agent_task_queue_events` — plus indexes for the hot reads.
//! The wire types are defined via `#[derive(architect::Entity)]`
//! in `agent-proto`; here we just spell the schema in SeaORM's
//! migration DSL.
//!
//! `queue_events` is hand-rolled (no architect entity) because
//! it's append-only, internal, and never crosses the RPC wire.

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260522_000001_create_agent_tasks::Migration)]
    }
}

mod m20260522_000001_create_agent_tasks {
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
            drop_table(manager, QueueEvents::Table).await?;
            drop_table(manager, AgentTaskComments::Table).await?;
            drop_table(manager, AgentTaskLinks::Table).await?;
            drop_table(manager, AgentTasks::Table).await?;
            drop_table(manager, AgentQueues::Table).await?;
            Ok(())
        }
    }

    async fn drop_table<T>(manager: &SchemaManager<'_>, table: T) -> Result<(), DbErr>
    where
        T: IntoIden + 'static,
    {
        manager
            .drop_table(Table::drop().table(table).if_exists().to_owned())
            .await
    }

    async fn create_tables(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
        // agent_queues
        manager
            .create_table(
                Table::create()
                    .table(AgentQueues::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AgentQueues::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AgentQueues::Label).string().not_null())
                    .col(
                        ColumnDef::new(AgentQueues::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AgentQueues::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // agent_tasks
        manager
            .create_table(
                Table::create()
                    .table(AgentTasks::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AgentTasks::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AgentTasks::QueueId).string().not_null())
                    .col(ColumnDef::new(AgentTasks::Title).string().not_null())
                    .col(ColumnDef::new(AgentTasks::Description).text().not_null())
                    .col(ColumnDef::new(AgentTasks::Status).string().not_null())
                    .col(ColumnDef::new(AgentTasks::Priority).integer().not_null())
                    .col(ColumnDef::new(AgentTasks::Labels).json().not_null())
                    .col(ColumnDef::new(AgentTasks::SourceTask).string().not_null())
                    .col(ColumnDef::new(AgentTasks::Project).string().not_null())
                    .col(ColumnDef::new(AgentTasks::AgentProfile).string().not_null())
                    .col(ColumnDef::new(AgentTasks::ClaimedBy).string().not_null())
                    .col(
                        ColumnDef::new(AgentTasks::ClaimExpiresAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(ColumnDef::new(AgentTasks::WorkerPid).integer().not_null())
                    .col(ColumnDef::new(AgentTasks::ResultBlob).text().not_null())
                    .col(
                        ColumnDef::new(AgentTasks::LinkedSessionId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AgentTasks::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AgentTasks::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AgentTasks::ArchivedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AgentTasks::Table, AgentTasks::QueueId)
                            .to(AgentQueues::Table, AgentQueues::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // agent_task_links
        manager
            .create_table(
                Table::create()
                    .table(AgentTaskLinks::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AgentTaskLinks::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AgentTaskLinks::FromTask).uuid().not_null())
                    .col(ColumnDef::new(AgentTaskLinks::ToTask).uuid().not_null())
                    .col(ColumnDef::new(AgentTaskLinks::Kind).string().not_null())
                    .col(
                        ColumnDef::new(AgentTaskLinks::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AgentTaskLinks::Table, AgentTaskLinks::FromTask)
                            .to(AgentTasks::Table, AgentTasks::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AgentTaskLinks::Table, AgentTaskLinks::ToTask)
                            .to(AgentTasks::Table, AgentTasks::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Composite uniqueness on (from_task, to_task, kind).
        manager
            .create_index(
                Index::create()
                    .name("ux_agent_task_links_triple")
                    .table(AgentTaskLinks::Table)
                    .col(AgentTaskLinks::FromTask)
                    .col(AgentTaskLinks::ToTask)
                    .col(AgentTaskLinks::Kind)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // agent_task_comments
        manager
            .create_table(
                Table::create()
                    .table(AgentTaskComments::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AgentTaskComments::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AgentTaskComments::AgentTaskId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AgentTaskComments::Author)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AgentTaskComments::Body).text().not_null())
                    .col(
                        ColumnDef::new(AgentTaskComments::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AgentTaskComments::Table, AgentTaskComments::AgentTaskId)
                            .to(AgentTasks::Table, AgentTasks::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // queue_events — internal, append-only, no architect entity.
        manager
            .create_table(
                Table::create()
                    .table(QueueEvents::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(QueueEvents::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(QueueEvents::QueueId).string().not_null())
                    .col(ColumnDef::new(QueueEvents::AgentTaskId).uuid().null())
                    .col(ColumnDef::new(QueueEvents::Kind).string().not_null())
                    .col(ColumnDef::new(QueueEvents::Payload).json().not_null())
                    .col(ColumnDef::new(QueueEvents::Ts).big_integer().not_null())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn create_indexes(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name("idx_agent_tasks_queue_status")
                    .table(AgentTasks::Table)
                    .col(AgentTasks::QueueId)
                    .col(AgentTasks::Status)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_agent_tasks_source_task")
                    .table(AgentTasks::Table)
                    .col(AgentTasks::SourceTask)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_queue_events_qid_id")
                    .table(QueueEvents::Table)
                    .col(QueueEvents::QueueId)
                    .col(QueueEvents::Id)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_comments_task")
                    .table(AgentTaskComments::Table)
                    .col(AgentTaskComments::AgentTaskId)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    // ── Idens ─────────────────────────────────────────────────────────

    #[derive(Iden)]
    enum AgentQueues {
        Table,
        Id,
        Label,
        CreatedAt,
        UpdatedAt,
    }

    #[derive(Iden)]
    enum AgentTasks {
        Table,
        Id,
        QueueId,
        Title,
        Description,
        Status,
        Priority,
        Labels,
        SourceTask,
        Project,
        AgentProfile,
        ClaimedBy,
        ClaimExpiresAt,
        WorkerPid,
        ResultBlob,
        LinkedSessionId,
        CreatedAt,
        UpdatedAt,
        ArchivedAt,
    }

    #[derive(Iden)]
    enum AgentTaskLinks {
        Table,
        Id,
        FromTask,
        ToTask,
        Kind,
        CreatedAt,
    }

    #[derive(Iden)]
    enum AgentTaskComments {
        Table,
        Id,
        AgentTaskId,
        Author,
        Body,
        CreatedAt,
    }

    #[derive(Iden)]
    enum QueueEvents {
        Table,
        Id,
        QueueId,
        AgentTaskId,
        Kind,
        Payload,
        Ts,
    }
}
