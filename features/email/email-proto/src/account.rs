//! Account identity. Backends serve one or more accounts; the
//! `vault_id` analogue here is [`AccountId`], an opaque string
//! the client uses to address one mailbox.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// Opaque per-account handle. The backend decides what shape
/// these take (UUID, email address, server-assigned id…); the
/// proto treats them as opaque strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
pub struct AccountId(pub String);

/// User-facing description of a mailbox. Backends return this
/// from configuration; clients use it to render account
/// pickers and pre-fill `From:` on compose.
#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    pub name: String,
    pub address: String,
    pub display_name: Option<String>,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{Account, AccountId};
    unsafe impl vox_types::Reborrow for AccountId {
        type Ref<'a> = AccountId;
    }
    unsafe impl vox_types::Reborrow for Account {
        type Ref<'a> = Account;
    }
}
