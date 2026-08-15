//! Per-member hourly rate rows.
//!
//! The full cascade, most-specific-first:
//!
//! 1. [`ProjectMemberRate`] — `(project_id, user_id)`.
//!    The hourly rate Alice gets billed at on the ACME
//!    redesign specifically.
//! 2. The project's `defaultRateCents` from its markdown
//!    frontmatter (read off disk via `features/project`).
//! 3. [`OrgMemberRate`] — `(org_id, user_id)`. The hourly
//!    rate Alice gets billed at *across* this org's
//!    projects, unless overridden.
//! 4. `0` (non-billable).
//!
//! `currency` lives on the project; we don't replicate it
//! here. A rate-cents value without an attached project is
//! meaningless. (Cross-currency conversion is a finances-
//! crate concern, not a timer one.)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq, Eq,
)]
#[architect(table_name = "timer_project_member_rates", repo)]
pub struct ProjectMemberRate {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Project (`features/project::ProjectInfo::id`).
    #[architect(filterable)]
    pub project_id: Uuid,

    /// Member user (`architect_auth::AuthUser::id`).
    #[architect(filterable)]
    pub user_id: Uuid,

    /// Cents per hour.
    pub hourly_cents: i64,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq, Eq,
)]
#[architect(table_name = "timer_org_member_rates", repo)]
pub struct OrgMemberRate {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Org (`architect_auth::AuthOrganization::id`).
    #[architect(filterable)]
    pub org_id: Uuid,

    /// Member user.
    #[architect(filterable)]
    pub user_id: Uuid,

    /// Cents per hour.
    pub hourly_cents: i64,

    /// ISO 4217. Default currency the member is billed in
    /// when an org-level rate kicks in. Project-level rates
    /// inherit the project's currency; this column gives the
    /// org-level fallback something to anchor against.
    pub currency: String,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}
