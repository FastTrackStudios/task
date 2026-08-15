//! Re-exports of the architect-emitted SeaORM items.

pub use identity_proto::linked_server::__linked_server_storage::{
    ActiveModel as LinkedServerActive, Column as LinkedServerColumn, Entity as LinkedServerEntity,
    Model as LinkedServerModel, Relation as LinkedServerRelation,
};

// Repo + storage glue.
pub use identity_proto::LinkedServer;
pub use identity_proto::linked_server::{
    LinkedServerCreate, LinkedServerRepo, LinkedServerRepoStorage, LinkedServerUpdate,
};
