//! Re-exports of the architect-emitted SeaORM items.
//!
//! `#[derive(architect::Entity)]` parks the SeaORM
//! `Entity`/`Model`/`Column`/`Relation`/`ActiveModel` items
//! inside a hidden `__<snake>_storage` module sibling to the
//! struct. This module surfaces them under sensible names for
//! callers in `agent-tasks`, the server's migration runner,
//! and downstream apps.

pub use agent_proto::tasks::__queue_storage::{
    ActiveModel as QueueActive, Column as QueueColumn, Entity as QueueEntity, Model as QueueModel,
    Relation as QueueRelation,
};

pub use agent_proto::tasks::__agent_task_storage::{
    ActiveModel as AgentTaskActive, Column as AgentTaskColumn, Entity as AgentTaskEntity,
    Model as AgentTaskModel, Relation as AgentTaskRelation,
};

pub use agent_proto::tasks::__agent_task_link_storage::{
    ActiveModel as AgentTaskLinkActive, Column as AgentTaskLinkColumn,
    Entity as AgentTaskLinkEntity, Model as AgentTaskLinkModel, Relation as AgentTaskLinkRelation,
};

pub use agent_proto::tasks::__agent_task_comment_storage::{
    ActiveModel as AgentTaskCommentActive, Column as AgentTaskCommentColumn,
    Entity as AgentTaskCommentEntity, Model as AgentTaskCommentModel,
    Relation as AgentTaskCommentRelation,
};

// Repo + storage glue from architect's emission.
pub use agent_proto::tasks::{
    AgentTask, AgentTaskComment, AgentTaskCommentCreate, AgentTaskCommentRepo,
    AgentTaskCommentRepoStorage, AgentTaskCommentUpdate, AgentTaskCreate, AgentTaskLink,
    AgentTaskLinkCreate, AgentTaskLinkRepo, AgentTaskLinkRepoStorage, AgentTaskLinkUpdate,
    AgentTaskRepo, AgentTaskRepoStorage, AgentTaskUpdate, Labels, Queue, QueueCreate, QueueRepo,
    QueueRepoStorage, QueueUpdate,
};
