//! The deterministic local-owner user id.
//!
//! Task has no membership sync yet: a single-user org has no row
//! that says "this human is user X". Every surface that needs a
//! `user_id` to scope work sessions therefore *derives* one from
//! the org id, and they must all derive the **same** one — the CLI
//! writes sessions the `/timer` page has to read back, and the
//! watch bridge writes sessions both of them have to see.
//!
//! This was previously three hand-copied `Uuid::new_v5(&org_id,
//! b"task-local-owner")` calls (`task-cli`'s timer module, the
//! server's watch bridge, and the web UI's chrome) whose doc
//! comments said they MUST match, with nothing enforcing it. One
//! divergent byte would have silently split a user's time tracking
//! into two invisible halves. It lives here, in the crate all three
//! already depend on, so there is exactly one definition.
//!
//! The value is load-bearing for on-disk data: changing the
//! namespace bytes orphans every existing work session.

use uuid::Uuid;

/// The v5 name hashed under the org id. Never change this — see
/// the module docs.
const LOCAL_OWNER_NAME: &[u8] = b"task-local-owner";

/// Deterministic local-owner user id for `org_id`.
///
/// Every Task surface that needs a `user_id` without a real
/// membership record must call this rather than re-deriving it.
#[must_use]
pub fn local_owner_id(org_id: Uuid) -> Uuid {
    Uuid::new_v5(&org_id, LOCAL_OWNER_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the derivation against accidental change. This value was
    /// produced by the three copies that existed before they were
    /// collapsed into this function, so it is what is already on disk
    /// in every user's timer db. A failure here means work sessions
    /// written by an older build become unreachable.
    #[test]
    fn local_owner_id_is_stable() {
        let org = Uuid::nil();
        assert_eq!(
            local_owner_id(org).to_string(),
            "634ef548-9a11-5a6d-8e1f-63173cdff06e",
            "the namespace name must stay `task-local-owner`"
        );
    }

    #[test]
    fn local_owner_id_is_deterministic_and_org_scoped() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert_eq!(local_owner_id(a), local_owner_id(a));
        assert_ne!(
            local_owner_id(a),
            local_owner_id(b),
            "two orgs must not collapse onto one owner"
        );
    }
}
