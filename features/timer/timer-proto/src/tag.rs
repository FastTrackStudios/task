//! `Tag` — labels on [`crate::WorkSession`]s. Many-to-many
//! via [`WorkSessionTag`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[architect(table_name = "timer_tags", repo)]
pub struct Tag {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable)]
    pub org_id: Uuid,

    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    /// Hex `#RRGGBB`. Empty = UI picks.
    pub color: String,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}

/// Join row — many-to-many between [`crate::WorkSession`] and
/// [`Tag`]. Surrogate primary key so architect's CRUD repo
/// works without a composite-key shim; uniqueness is enforced
/// by an index on `(work_session_id, tag_id)`.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq, Eq,
)]
#[architect(table_name = "timer_work_session_tags", repo)]
pub struct WorkSessionTag {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,
    #[architect(filterable)]
    pub work_session_id: Uuid,
    #[architect(filterable)]
    pub tag_id: Uuid,
    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
}
