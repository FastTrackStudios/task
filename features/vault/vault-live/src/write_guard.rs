//! Who may write a vault file — a question the host answers.
//!
//! The backend knows files, not people. Some shelves are governed by
//! rules the vault cannot see: a wiki whose Editors have been declared
//! takes direct writes from its Editors only, and everyone else's change
//! goes through an Edit Request (`wiki.edit.editor`). The web editor
//! saves a wiki page through [`VaultSync`](vault_proto::VaultSync), not
//! through the wiki lane, so that rule has to hold here too — or the
//! editor would be a way around it.
//!
//! So the host hands the backend a [`WriteGuard`], and the backend asks
//! it before every write a caller can make: `put_file`, `delete_file`,
//! `set_folder`, and `open_collab` (joining a file's live session is how
//! the collaborative editor writes).
//!
//! # Knowing who asked
//!
//! The permissions gate records the caller in a task-local, and the
//! backend runs its sync methods on tokio's blocking pool, where that
//! task-local is gone. [`CallerDispatcher`] reads the caller on the
//! request task, before the hop, and carries it onto the blocking thread
//! for the length of the call; [`current_caller`] reads it back.
//!
//! A call with no caller is the server acting on its own behalf — the
//! collab write-behind flushing a document, a seed, a migration — and a
//! guard lets it through, the same rule the wiki lane applies: the lane
//! governs *people*, and the server is not one.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;

use architect::dispatch::{BoxedAny, DispatchError, Dispatcher};

/// The host's answer to "may this caller write this file?".
pub trait WriteGuard: Send + Sync + 'static {
    /// `Ok` to allow; `Err` with the reason to refuse. `principal` is the
    /// account the gate resolved, or `None` for an in-process call.
    fn check(&self, vault_id: &str, path: &str, principal: Option<&str>) -> Result<(), String>;
}

thread_local! {
    static CALLER: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The account whose call is running on this thread, as
/// [`CallerDispatcher`] carried it here. `None` off a dispatched call.
#[must_use]
pub fn current_caller() -> Option<String> {
    CALLER.with(|c| c.borrow().clone())
}

/// The account the permissions gate resolved for the call on this task.
#[must_use]
pub fn gate_principal() -> Option<String> {
    match architect::permissions_gate::caller() {
        Some(architect_permissions::Principal::User { user_id }) => Some(user_id),
        _ => None,
    }
}

/// Clears the carried caller when the call ends, however it ends.
struct CallerScope(Option<String>);

impl CallerScope {
    fn enter(who: Option<String>) -> Self {
        Self(CALLER.with(|c| c.replace(who)))
    }
}

impl Drop for CallerScope {
    fn drop(&mut self) {
        let previous = self.0.take();
        CALLER.with(|c| *c.borrow_mut() = previous);
    }
}

/// `TokioBlockingDispatcher`, carrying the gate's caller across the hop.
#[derive(Default)]
pub struct CallerDispatcher;

impl Dispatcher for CallerDispatcher {
    fn dispatch(
        &self,
        f: Box<dyn FnOnce() -> BoxedAny + Send + 'static>,
    ) -> Pin<Box<dyn Future<Output = Result<BoxedAny, DispatchError>> + Send + 'static>> {
        // Read on the request task, where the gate set it.
        let who = gate_principal();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let _scope = CallerScope::enter(who);
                f()
            })
            .await
            .map_err(|join_err| {
                if join_err.is_panic() {
                    DispatchError::Panicked(format!("{join_err:?}"))
                } else {
                    DispatchError::Cancelled
                }
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_carries_the_caller_and_restores_what_was_there() {
        assert_eq!(current_caller(), None);
        {
            let _outer = CallerScope::enter(Some("alice".into()));
            assert_eq!(current_caller().as_deref(), Some("alice"));
            {
                let _inner = CallerScope::enter(Some("sam".into()));
                assert_eq!(current_caller().as_deref(), Some("sam"));
            }
            assert_eq!(current_caller().as_deref(), Some("alice"));
        }
        assert_eq!(current_caller(), None);
    }
}
