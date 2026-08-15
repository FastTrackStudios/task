//! Re-exports of the architect-emitted SeaORM items.

pub use threads_proto::message::__message_storage::{
    ActiveModel as MessageActive, Column as MessageColumn, Entity as MessageEntity,
    Model as MessageModel, Relation as MessageRelation,
};
pub use threads_proto::thread::__thread_storage::{
    ActiveModel as ThreadActive, Column as ThreadColumn, Entity as ThreadEntity,
    Model as ThreadModel, Relation as ThreadRelation,
};

// Repo + storage glue (architect-emitted), exposed for callers that want
// plain CRUD alongside the domain `ThreadsService`.
pub use threads_proto::message::{MessageCreate, MessageRepo, MessageRepoStorage, MessageUpdate};
pub use threads_proto::thread::{ThreadCreate, ThreadRepo, ThreadRepoStorage, ThreadUpdate};
pub use threads_proto::{Message, Thread};
