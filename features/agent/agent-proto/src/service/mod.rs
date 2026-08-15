//! Per-capability traits for the agent feature.
//!
//! Each trait is decorated with `#[architect::rpc]` and
//! emits its own async client + dispatcher + descriptor.
//! Backends implement only the traits they support —
//! Codex, for instance, implements [`sessions::Sessions`] +
//! [`turn_dispatch::TurnDispatch`] +
//! [`threads::Threads`] + [`subscriptions::Subscriptions`] +
//! [`external_import::ExternalImport`], and ignores the
//! rest. Callers express what they need via trait bounds:
//!
//! ```ignore
//! fn dispatch_chat<A: Sessions + TurnDispatch + Subscriptions>(
//!     agent: &A,
//!     /* ... */
//! ) { /* ... */ }
//! ```
//!
//! There is no umbrella `Agents` trait. Code that needs
//! several capabilities lists them in its bounds — the
//! type signature is the documentation.

pub mod approvals;
pub mod attachments;
pub mod backends;
pub mod discovery;
pub mod external_import;
pub mod profiles;
pub mod projects;
pub mod questions;
pub mod reasoning;
pub mod routines;
pub mod run_stream;
pub mod runs;
pub mod sessions;
pub mod subscriptions;
pub mod tasks;
pub mod threads;
pub mod tool_calls;
pub mod turn_dispatch;

pub use approvals::Approvals;
pub use attachments::Attachments;
pub use backends::Backends;
pub use external_import::ExternalImport;
pub use profiles::Profiles;
pub use projects::Projects;
pub use questions::Questions;
pub use reasoning::Reasoning;
pub use routines::{NewRoutine, Routine, Routines};
pub use sessions::{CreateSession, SessionFilter, SessionPage, Sessions};
pub use subscriptions::Subscriptions;
pub use tasks::AgentTaskQueue;
pub use threads::Threads;
pub use tool_calls::ToolCalls;
pub use turn_dispatch::{DispatchAck, DispatchTurn, TurnDispatch};
