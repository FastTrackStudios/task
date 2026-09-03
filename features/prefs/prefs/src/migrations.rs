//! SeaORM migrator for the prefs schema.

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260702_000001_create_prefs::Migration),
            Box::new(m20260715_000002_add_theme::Migration),
            Box::new(m20260716_000003_add_shortcuts_priority::Migration),
            Box::new(m20260902_000004_add_vim_mode::Migration),
        ]
    }
}

mod m20260702_000001_create_prefs {
    use sea_orm_migration::prelude::*;

    #[derive(DeriveMigrationName)]
    pub struct Migration;

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .create_table(
                    Table::create()
                        .table(UserPrefs::Table)
                        .if_not_exists()
                        .col(
                            ColumnDef::new(UserPrefs::UserId)
                                .uuid()
                                .not_null()
                                .primary_key(),
                        )
                        .col(ColumnDef::new(UserPrefs::DefaultPage).string().not_null())
                        .col(ColumnDef::new(UserPrefs::TasksActive).boolean().not_null())
                        .col(
                            ColumnDef::new(UserPrefs::TasksRelevant)
                                .boolean()
                                .not_null(),
                        )
                        .col(ColumnDef::new(UserPrefs::Location).string().not_null())
                        .to_owned(),
                )
                .await
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .drop_table(Table::drop().table(UserPrefs::Table).if_exists().to_owned())
                .await
        }
    }

    // ── Iden ──────────────────────────────────────────────────────────

    #[derive(Iden)]
    enum UserPrefs {
        Table,
        UserId,
        DefaultPage,
        TasksActive,
        TasksRelevant,
        Location,
    }
}

mod m20260715_000002_add_theme {
    use sea_orm_migration::prelude::*;

    pub struct Migration;

    // Manual name: `DeriveMigrationName` stamps the *file* stem, and
    // both migrations live inline in migrations.rs — the derive would
    // collide with the create migration's already-deployed
    // "migrations" version row.
    impl MigrationName for Migration {
        fn name(&self) -> &str {
            "m20260715_000002_add_theme"
        }
    }

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            // Two ALTERs — SQLite takes one ADD COLUMN per statement.
            // Empty-string default = "never picked" (matches
            // `UserPrefs::defaults_for`).
            manager
                .alter_table(
                    Table::alter()
                        .table(UserPrefs::Table)
                        .add_column(
                            ColumnDef::new(UserPrefs::ThemePreset)
                                .string()
                                .not_null()
                                .default(""),
                        )
                        .to_owned(),
                )
                .await?;
            manager
                .alter_table(
                    Table::alter()
                        .table(UserPrefs::Table)
                        .add_column(
                            ColumnDef::new(UserPrefs::ThemeMode)
                                .string()
                                .not_null()
                                .default(""),
                        )
                        .to_owned(),
                )
                .await
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .alter_table(
                    Table::alter()
                        .table(UserPrefs::Table)
                        .drop_column(UserPrefs::ThemePreset)
                        .to_owned(),
                )
                .await?;
            manager
                .alter_table(
                    Table::alter()
                        .table(UserPrefs::Table)
                        .drop_column(UserPrefs::ThemeMode)
                        .to_owned(),
                )
                .await
        }
    }

    // ── Iden ──────────────────────────────────────────────────────────

    #[derive(Iden)]
    enum UserPrefs {
        Table,
        ThemePreset,
        ThemeMode,
    }
}

mod m20260716_000003_add_shortcuts_priority {
    use sea_orm_migration::prelude::*;

    pub struct Migration;

    // Manual name for the same reason as the theme migration: every
    // migration lives inline in migrations.rs, so the file-stem derive
    // would collide.
    impl MigrationName for Migration {
        fn name(&self) -> &str {
            "m20260716_000003_add_shortcuts_priority"
        }
    }

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            // Default TRUE = "prioritize app shortcuts" on — matches
            // `UserPrefs::defaults_for`.
            manager
                .alter_table(
                    Table::alter()
                        .table(UserPrefs::Table)
                        .add_column(
                            ColumnDef::new(UserPrefs::ShortcutsPriority)
                                .boolean()
                                .not_null()
                                .default(true),
                        )
                        .to_owned(),
                )
                .await
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .alter_table(
                    Table::alter()
                        .table(UserPrefs::Table)
                        .drop_column(UserPrefs::ShortcutsPriority)
                        .to_owned(),
                )
                .await
        }
    }

    // ── Iden ──────────────────────────────────────────────────────────

    #[derive(Iden)]
    enum UserPrefs {
        Table,
        ShortcutsPriority,
    }
}

mod m20260902_000004_add_vim_mode {
    use sea_orm_migration::prelude::*;

    pub struct Migration;

    // Manual name for the same reason as the migrations above: every
    // migration lives inline in migrations.rs, so the file-stem derive
    // would collide.
    impl MigrationName for Migration {
        fn name(&self) -> &str {
            "m20260902_000004_add_vim_mode"
        }
    }

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            // Default FALSE = modal editing off — matches
            // `UserPrefs::defaults_for`. Existing rows adopt the new
            // default, which is deliberate: vim was never opt-in
            // before, so nobody chose it.
            manager
                .alter_table(
                    Table::alter()
                        .table(UserPrefs::Table)
                        .add_column(
                            ColumnDef::new(UserPrefs::VimMode)
                                .boolean()
                                .not_null()
                                .default(false),
                        )
                        .to_owned(),
                )
                .await
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .alter_table(
                    Table::alter()
                        .table(UserPrefs::Table)
                        .drop_column(UserPrefs::VimMode)
                        .to_owned(),
                )
                .await
        }
    }

    // ── Iden ──────────────────────────────────────────────────────────

    #[derive(Iden)]
    enum UserPrefs {
        Table,
        VimMode,
    }
}
