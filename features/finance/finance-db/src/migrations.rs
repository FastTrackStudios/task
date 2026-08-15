//! SeaORM migrator. Single migration creates all finance
//! tables. Future schema changes append new
//! `m<YYYYMMDD>_<NNN>_<topic>` modules.

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260524_000001_create_finance::Migration)]
    }
}

mod m20260524_000001_create_finance {
    use sea_orm_migration::prelude::*;

    #[derive(DeriveMigrationName)]
    pub struct Migration;

    #[async_trait::async_trait]
    impl MigrationTrait for Migration {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            books(manager).await?;
            accounts(manager).await?;
            transactions(manager).await?;
            parties(manager).await?;
            invoices(manager).await?;
            payments(manager).await?;
            invoice_payments(manager).await?;
            recurring_schedules(manager).await?;
            expenses(manager).await?;
            tax_rate_catalog(manager).await?;
            indexes(manager).await
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            for table in [
                FinanceTaxRateCatalog::Table.into_iden(),
                FinanceExpenses::Table.into_iden(),
                FinanceRecurringSchedules::Table.into_iden(),
                FinanceInvoicePayments::Table.into_iden(),
                FinancePayments::Table.into_iden(),
                FinanceInvoices::Table.into_iden(),
                FinanceParties::Table.into_iden(),
                FinanceTransactions::Table.into_iden(),
                FinanceAccounts::Table.into_iden(),
                FinanceBooks::Table.into_iden(),
            ] {
                manager
                    .drop_table(Table::drop().table(table).if_exists().to_owned())
                    .await?;
            }
            Ok(())
        }
    }

    async fn books(m: &SchemaManager<'_>) -> Result<(), DbErr> {
        m.create_table(
            Table::create()
                .table(FinanceBooks::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(FinanceBooks::Id)
                        .uuid()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(FinanceBooks::Name).string().not_null())
                .col(ColumnDef::new(FinanceBooks::Kind).string().not_null())
                .col(
                    ColumnDef::new(FinanceBooks::BaseCurrency)
                        .string()
                        .not_null(),
                )
                .col(ColumnDef::new(FinanceBooks::SettingsJson).text().not_null())
                .col(
                    ColumnDef::new(FinanceBooks::CreatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceBooks::UpdatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .to_owned(),
        )
        .await
    }

    async fn accounts(m: &SchemaManager<'_>) -> Result<(), DbErr> {
        m.create_table(
            Table::create()
                .table(FinanceAccounts::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(FinanceAccounts::Id)
                        .uuid()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(FinanceAccounts::BookId).uuid().not_null())
                .col(ColumnDef::new(FinanceAccounts::Name).string().not_null())
                .col(ColumnDef::new(FinanceAccounts::Kind).string().not_null())
                .col(ColumnDef::new(FinanceAccounts::ParentId).uuid().not_null())
                .col(
                    ColumnDef::new(FinanceAccounts::Currency)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceAccounts::IsArchived)
                        .boolean()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceAccounts::OpeningBalanceMinor)
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(FinanceAccounts::Notes).text().not_null())
                .col(
                    ColumnDef::new(FinanceAccounts::CreatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceAccounts::UpdatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(FinanceAccounts::Table, FinanceAccounts::BookId)
                        .to(FinanceBooks::Table, FinanceBooks::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await
    }

    async fn transactions(m: &SchemaManager<'_>) -> Result<(), DbErr> {
        m.create_table(
            Table::create()
                .table(FinanceTransactions::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(FinanceTransactions::Id)
                        .uuid()
                        .not_null()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(FinanceTransactions::BookId)
                        .uuid()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceTransactions::Date)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceTransactions::Description)
                        .text()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceTransactions::Reference)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceTransactions::SourceKind)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceTransactions::SourceId)
                        .uuid()
                        .not_null(),
                )
                // Splits ride as a JSON column on the header
                // row — matches the proto's `TransactionSplits`
                // newtype. Sum-to-zero is enforced by the
                // service layer, not the DB.
                .col(
                    ColumnDef::new(FinanceTransactions::Splits)
                        .json()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceTransactions::CreatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(FinanceTransactions::Table, FinanceTransactions::BookId)
                        .to(FinanceBooks::Table, FinanceBooks::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await
    }

    async fn parties(m: &SchemaManager<'_>) -> Result<(), DbErr> {
        m.create_table(
            Table::create()
                .table(FinanceParties::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(FinanceParties::Id)
                        .uuid()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(FinanceParties::BookId).uuid().not_null())
                .col(ColumnDef::new(FinanceParties::Kind).string().not_null())
                .col(
                    ColumnDef::new(FinanceParties::DisplayName)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceParties::LegalName)
                        .string()
                        .not_null(),
                )
                .col(ColumnDef::new(FinanceParties::Email).string().not_null())
                .col(ColumnDef::new(FinanceParties::Phone).string().not_null())
                .col(ColumnDef::new(FinanceParties::Address).text().not_null())
                .col(ColumnDef::new(FinanceParties::TaxId).string().not_null())
                .col(
                    ColumnDef::new(FinanceParties::DefaultCurrency)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceParties::DefaultNetDays)
                        .integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceParties::DefaultRateMinorPerHour)
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(FinanceParties::Notes).text().not_null())
                .col(
                    ColumnDef::new(FinanceParties::IsArchived)
                        .boolean()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceParties::CreatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceParties::UpdatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(FinanceParties::Table, FinanceParties::BookId)
                        .to(FinanceBooks::Table, FinanceBooks::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await
    }

    async fn invoices(m: &SchemaManager<'_>) -> Result<(), DbErr> {
        m.create_table(
            Table::create()
                .table(FinanceInvoices::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(FinanceInvoices::Id)
                        .uuid()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(FinanceInvoices::BookId).uuid().not_null())
                .col(ColumnDef::new(FinanceInvoices::PartyId).uuid().not_null())
                .col(ColumnDef::new(FinanceInvoices::Kind).string().not_null())
                .col(ColumnDef::new(FinanceInvoices::Number).string().not_null())
                .col(ColumnDef::new(FinanceInvoices::Status).string().not_null())
                .col(
                    ColumnDef::new(FinanceInvoices::IssueDate)
                        .string()
                        .not_null(),
                )
                .col(ColumnDef::new(FinanceInvoices::DueDate).string().not_null())
                .col(
                    ColumnDef::new(FinanceInvoices::Currency)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoices::ExchangeRateMicro)
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(FinanceInvoices::LineItems).json().not_null())
                .col(
                    ColumnDef::new(FinanceInvoices::InvoiceTaxes)
                        .json()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoices::UsesInclusiveTaxes)
                        .boolean()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoices::SubtotalMinor)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoices::TaxTotalMinor)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoices::TotalMinor)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoices::AmountPaidMinor)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoices::BalanceMinor)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoices::NotesPublic)
                        .text()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoices::NotesPrivate)
                        .text()
                        .not_null(),
                )
                .col(ColumnDef::new(FinanceInvoices::Terms).text().not_null())
                .col(ColumnDef::new(FinanceInvoices::Footer).text().not_null())
                .col(ColumnDef::new(FinanceInvoices::Locked).boolean().not_null())
                .col(
                    ColumnDef::new(FinanceInvoices::PostedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoices::CreatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoices::UpdatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(FinanceInvoices::Table, FinanceInvoices::BookId)
                        .to(FinanceBooks::Table, FinanceBooks::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(FinanceInvoices::Table, FinanceInvoices::PartyId)
                        .to(FinanceParties::Table, FinanceParties::Id)
                        .on_delete(ForeignKeyAction::Restrict),
                )
                .to_owned(),
        )
        .await
    }

    async fn payments(m: &SchemaManager<'_>) -> Result<(), DbErr> {
        m.create_table(
            Table::create()
                .table(FinancePayments::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(FinancePayments::Id)
                        .uuid()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(FinancePayments::BookId).uuid().not_null())
                .col(ColumnDef::new(FinancePayments::PartyId).uuid().not_null())
                .col(ColumnDef::new(FinancePayments::Date).string().not_null())
                .col(
                    ColumnDef::new(FinancePayments::AmountMinor)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinancePayments::Currency)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinancePayments::ExchangeRateMicro)
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(FinancePayments::Method).string().not_null())
                .col(
                    ColumnDef::new(FinancePayments::Reference)
                        .string()
                        .not_null(),
                )
                .col(ColumnDef::new(FinancePayments::Status).string().not_null())
                .col(
                    ColumnDef::new(FinancePayments::AppliedMinor)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinancePayments::RefundedMinor)
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(FinancePayments::Notes).text().not_null())
                .col(
                    ColumnDef::new(FinancePayments::CreatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(FinancePayments::Table, FinancePayments::BookId)
                        .to(FinanceBooks::Table, FinanceBooks::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await
    }

    async fn invoice_payments(m: &SchemaManager<'_>) -> Result<(), DbErr> {
        m.create_table(
            Table::create()
                .table(FinanceInvoicePayments::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(FinanceInvoicePayments::Id)
                        .uuid()
                        .not_null()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(FinanceInvoicePayments::PaymentId)
                        .uuid()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoicePayments::InvoiceId)
                        .uuid()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoicePayments::AmountMinor)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoicePayments::RefundedMinor)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceInvoicePayments::CreatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(
                            FinanceInvoicePayments::Table,
                            FinanceInvoicePayments::PaymentId,
                        )
                        .to(FinancePayments::Table, FinancePayments::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(
                            FinanceInvoicePayments::Table,
                            FinanceInvoicePayments::InvoiceId,
                        )
                        .to(FinanceInvoices::Table, FinanceInvoices::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await
    }

    async fn recurring_schedules(m: &SchemaManager<'_>) -> Result<(), DbErr> {
        m.create_table(
            Table::create()
                .table(FinanceRecurringSchedules::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(FinanceRecurringSchedules::Id)
                        .uuid()
                        .not_null()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(FinanceRecurringSchedules::BookId)
                        .uuid()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceRecurringSchedules::TemplateInvoiceId)
                        .uuid()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceRecurringSchedules::Frequency)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceRecurringSchedules::Status)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceRecurringSchedules::StartDate)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceRecurringSchedules::NextRunAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceRecurringSchedules::RemainingCycles)
                        .integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceRecurringSchedules::LastRunAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceRecurringSchedules::CreatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(
                            FinanceRecurringSchedules::Table,
                            FinanceRecurringSchedules::BookId,
                        )
                        .to(FinanceBooks::Table, FinanceBooks::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(
                            FinanceRecurringSchedules::Table,
                            FinanceRecurringSchedules::TemplateInvoiceId,
                        )
                        .to(FinanceInvoices::Table, FinanceInvoices::Id)
                        .on_delete(ForeignKeyAction::Restrict),
                )
                .to_owned(),
        )
        .await
    }

    async fn expenses(m: &SchemaManager<'_>) -> Result<(), DbErr> {
        m.create_table(
            Table::create()
                .table(FinanceExpenses::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(FinanceExpenses::Id)
                        .uuid()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(FinanceExpenses::BookId).uuid().not_null())
                .col(ColumnDef::new(FinanceExpenses::PartyId).uuid().not_null())
                .col(
                    ColumnDef::new(FinanceExpenses::CategoryAccountId)
                        .uuid()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceExpenses::PaymentAccountId)
                        .uuid()
                        .not_null(),
                )
                .col(ColumnDef::new(FinanceExpenses::Date).string().not_null())
                .col(
                    ColumnDef::new(FinanceExpenses::AmountMinor)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceExpenses::Currency)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceExpenses::ExchangeRateMicro)
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(FinanceExpenses::Taxes).json().not_null())
                .col(
                    ColumnDef::new(FinanceExpenses::Reference)
                        .string()
                        .not_null(),
                )
                .col(ColumnDef::new(FinanceExpenses::Notes).text().not_null())
                .col(
                    ColumnDef::new(FinanceExpenses::BillableToInvoiceId)
                        .uuid()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceExpenses::ReceiptAttachmentId)
                        .uuid()
                        .not_null(),
                )
                .col(ColumnDef::new(FinanceExpenses::Tags).json().not_null())
                .col(
                    ColumnDef::new(FinanceExpenses::CreatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceExpenses::UpdatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(FinanceExpenses::Table, FinanceExpenses::BookId)
                        .to(FinanceBooks::Table, FinanceBooks::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await
    }

    async fn tax_rate_catalog(m: &SchemaManager<'_>) -> Result<(), DbErr> {
        m.create_table(
            Table::create()
                .table(FinanceTaxRateCatalog::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(FinanceTaxRateCatalog::Id)
                        .uuid()
                        .not_null()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(FinanceTaxRateCatalog::BookId)
                        .uuid()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceTaxRateCatalog::Name)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceTaxRateCatalog::RateMicro)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(FinanceTaxRateCatalog::CreatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(FinanceTaxRateCatalog::Table, FinanceTaxRateCatalog::BookId)
                        .to(FinanceBooks::Table, FinanceBooks::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await
    }

    async fn indexes(m: &SchemaManager<'_>) -> Result<(), DbErr> {
        // Hot reads: scope-by-book on every entity.
        for (name, table, col) in [
            (
                "idx_finance_accounts_book",
                FinanceAccounts::Table.into_iden(),
                FinanceAccounts::BookId.into_iden(),
            ),
            (
                "idx_finance_transactions_book",
                FinanceTransactions::Table.into_iden(),
                FinanceTransactions::BookId.into_iden(),
            ),
            (
                "idx_finance_parties_book",
                FinanceParties::Table.into_iden(),
                FinanceParties::BookId.into_iden(),
            ),
            (
                "idx_finance_invoices_book",
                FinanceInvoices::Table.into_iden(),
                FinanceInvoices::BookId.into_iden(),
            ),
            (
                "idx_finance_payments_book",
                FinancePayments::Table.into_iden(),
                FinancePayments::BookId.into_iden(),
            ),
            (
                "idx_finance_expenses_book",
                FinanceExpenses::Table.into_iden(),
                FinanceExpenses::BookId.into_iden(),
            ),
        ] {
            m.create_index(Index::create().name(name).table(table).col(col).to_owned())
                .await?;
        }
        // Unique invoice number per book — invoice numbers
        // ("INV-2026-0042") must not duplicate within one
        // ledger.
        m.create_index(
            Index::create()
                .name("ux_finance_invoices_book_number")
                .table(FinanceInvoices::Table)
                .col(FinanceInvoices::BookId)
                .col(FinanceInvoices::Number)
                .unique()
                .to_owned(),
        )
        .await?;
        Ok(())
    }

    // ── Iden ──────────────────────────────────────────────────────

    #[derive(Iden)]
    enum FinanceBooks {
        Table,
        Id,
        Name,
        Kind,
        BaseCurrency,
        SettingsJson,
        CreatedAt,
        UpdatedAt,
    }

    #[derive(Iden)]
    enum FinanceAccounts {
        Table,
        Id,
        BookId,
        Name,
        Kind,
        ParentId,
        Currency,
        IsArchived,
        OpeningBalanceMinor,
        Notes,
        CreatedAt,
        UpdatedAt,
    }

    #[derive(Iden)]
    enum FinanceTransactions {
        Table,
        Id,
        BookId,
        Date,
        Description,
        Reference,
        SourceKind,
        SourceId,
        Splits,
        CreatedAt,
    }

    #[derive(Iden)]
    enum FinanceParties {
        Table,
        Id,
        BookId,
        Kind,
        DisplayName,
        LegalName,
        Email,
        Phone,
        Address,
        TaxId,
        DefaultCurrency,
        DefaultNetDays,
        DefaultRateMinorPerHour,
        Notes,
        IsArchived,
        CreatedAt,
        UpdatedAt,
    }

    #[derive(Iden)]
    enum FinanceInvoices {
        Table,
        Id,
        BookId,
        PartyId,
        Kind,
        Number,
        Status,
        IssueDate,
        DueDate,
        Currency,
        ExchangeRateMicro,
        LineItems,
        InvoiceTaxes,
        UsesInclusiveTaxes,
        SubtotalMinor,
        TaxTotalMinor,
        TotalMinor,
        AmountPaidMinor,
        BalanceMinor,
        NotesPublic,
        NotesPrivate,
        Terms,
        Footer,
        Locked,
        PostedAt,
        CreatedAt,
        UpdatedAt,
    }

    #[derive(Iden)]
    enum FinancePayments {
        Table,
        Id,
        BookId,
        PartyId,
        Date,
        AmountMinor,
        Currency,
        ExchangeRateMicro,
        Method,
        Reference,
        Status,
        AppliedMinor,
        RefundedMinor,
        Notes,
        CreatedAt,
    }

    #[derive(Iden)]
    enum FinanceInvoicePayments {
        Table,
        Id,
        PaymentId,
        InvoiceId,
        AmountMinor,
        RefundedMinor,
        CreatedAt,
    }

    #[derive(Iden)]
    enum FinanceRecurringSchedules {
        Table,
        Id,
        BookId,
        TemplateInvoiceId,
        Frequency,
        Status,
        StartDate,
        NextRunAt,
        RemainingCycles,
        LastRunAt,
        CreatedAt,
    }

    #[derive(Iden)]
    enum FinanceExpenses {
        Table,
        Id,
        BookId,
        PartyId,
        CategoryAccountId,
        PaymentAccountId,
        Date,
        AmountMinor,
        Currency,
        ExchangeRateMicro,
        Taxes,
        Reference,
        Notes,
        BillableToInvoiceId,
        ReceiptAttachmentId,
        Tags,
        CreatedAt,
        UpdatedAt,
    }

    #[derive(Iden)]
    enum FinanceTaxRateCatalog {
        Table,
        Id,
        BookId,
        Name,
        RateMicro,
        CreatedAt,
    }
}
