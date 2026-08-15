//! Re-exports of the architect-emitted SeaORM items.

pub use timer_proto::client::__client_storage::{
    ActiveModel as ClientActive, Column as ClientColumn, Entity as ClientEntity,
    Model as ClientModel, Relation as ClientRelation,
};
pub use timer_proto::rate::__org_member_rate_storage::{
    ActiveModel as OrgMemberRateActive, Column as OrgMemberRateColumn,
    Entity as OrgMemberRateEntity, Model as OrgMemberRateModel, Relation as OrgMemberRateRelation,
};
pub use timer_proto::rate::__project_member_rate_storage::{
    ActiveModel as ProjectMemberRateActive, Column as ProjectMemberRateColumn,
    Entity as ProjectMemberRateEntity, Model as ProjectMemberRateModel,
    Relation as ProjectMemberRateRelation,
};
pub use timer_proto::session::__work_session_storage::{
    ActiveModel as WorkSessionActive, Column as WorkSessionColumn, Entity as WorkSessionEntity,
    Model as WorkSessionModel, Relation as WorkSessionRelation,
};
pub use timer_proto::tag::__tag_storage::{
    ActiveModel as TagActive, Column as TagColumn, Entity as TagEntity, Model as TagModel,
    Relation as TagRelation,
};
pub use timer_proto::tag::__work_session_tag_storage::{
    ActiveModel as WorkSessionTagActive, Column as WorkSessionTagColumn,
    Entity as WorkSessionTagEntity, Model as WorkSessionTagModel,
    Relation as WorkSessionTagRelation,
};

// Repo + storage glue, one set per entity-bearing module.
pub use timer_proto::client::{ClientCreate, ClientRepo, ClientRepoStorage, ClientUpdate};
pub use timer_proto::rate::{
    OrgMemberRateCreate, OrgMemberRateRepo, OrgMemberRateRepoStorage, OrgMemberRateUpdate,
    ProjectMemberRateCreate, ProjectMemberRateRepo, ProjectMemberRateRepoStorage,
    ProjectMemberRateUpdate,
};
pub use timer_proto::session::{
    WorkSessionCreate, WorkSessionRepo, WorkSessionRepoStorage, WorkSessionUpdate,
};
pub use timer_proto::tag::{
    TagCreate, TagRepo, TagRepoStorage, TagUpdate, WorkSessionTagCreate, WorkSessionTagRepo,
    WorkSessionTagRepoStorage, WorkSessionTagUpdate,
};
pub use timer_proto::{
    Client, OrgMemberRate, ProjectMemberRate, Tag, WorkSession, WorkSessionFilter, WorkSessionTag,
};
