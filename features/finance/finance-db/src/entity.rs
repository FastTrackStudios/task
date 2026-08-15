//! Re-exports of the architect-emitted SeaORM items.
//!
//! `#[derive(architect::Entity)]` parks each entity's
//! `Entity`/`Model`/`Column`/`Relation`/`ActiveModel` inside
//! a hidden `__<snake>_storage` module sibling to the
//! struct. The aliases below surface them under sensible
//! names: `BookEntity`, `AccountActive`, `InvoiceColumn`, etc.

pub use finance_proto::book::__book_storage::{
    ActiveModel as BookActive, Column as BookColumn, Entity as BookEntity, Model as BookModel,
    Relation as BookRelation,
};
pub use finance_proto::expense::__expense_storage::{
    ActiveModel as ExpenseActive, Column as ExpenseColumn, Entity as ExpenseEntity,
    Model as ExpenseModel, Relation as ExpenseRelation,
};
pub use finance_proto::invoice::__invoice_storage::{
    ActiveModel as InvoiceActive, Column as InvoiceColumn, Entity as InvoiceEntity,
    Model as InvoiceModel, Relation as InvoiceRelation,
};
pub use finance_proto::ledger::__account_storage::{
    ActiveModel as AccountActive, Column as AccountColumn, Entity as AccountEntity,
    Model as AccountModel, Relation as AccountRelation,
};
pub use finance_proto::ledger::__transaction_storage::{
    ActiveModel as TransactionActive, Column as TransactionColumn, Entity as TransactionEntity,
    Model as TransactionModel, Relation as TransactionRelation,
};
pub use finance_proto::party::__party_storage::{
    ActiveModel as PartyActive, Column as PartyColumn, Entity as PartyEntity, Model as PartyModel,
    Relation as PartyRelation,
};
pub use finance_proto::payment::__invoice_payment_storage::{
    ActiveModel as InvoicePaymentActive, Column as InvoicePaymentColumn,
    Entity as InvoicePaymentEntity, Model as InvoicePaymentModel,
    Relation as InvoicePaymentRelation,
};
pub use finance_proto::payment::__payment_storage::{
    ActiveModel as PaymentActive, Column as PaymentColumn, Entity as PaymentEntity,
    Model as PaymentModel, Relation as PaymentRelation,
};
pub use finance_proto::recurring::__recurring_schedule_storage::{
    ActiveModel as RecurringScheduleActive, Column as RecurringScheduleColumn,
    Entity as RecurringScheduleEntity, Model as RecurringScheduleModel,
    Relation as RecurringScheduleRelation,
};
pub use finance_proto::tax::__tax_rate_catalog_entry_storage::{
    ActiveModel as TaxRateCatalogEntryActive, Column as TaxRateCatalogEntryColumn,
    Entity as TaxRateCatalogEntryEntity, Model as TaxRateCatalogEntryModel,
    Relation as TaxRateCatalogEntryRelation,
};
