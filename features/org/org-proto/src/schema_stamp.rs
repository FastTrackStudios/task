//! Per-service schema stamps — the dev guard against proto /
//! server skew.
//!
//! A vox `MethodId` is a hash of the service name, method name,
//! arg shapes, and return shape, so XOR-folding a service's
//! method ids yields a stamp that changes whenever *any* method
//! is added, removed, or has its payload shape touched — exactly
//! the changes that produce `structural mismatch` /
//! `InvalidPayload` / `Unknown method` errors when a running
//! `task-server` binary predates a `*-proto` edit.
//!
//! - The **server** publishes its stamps in
//!   `/.well-known/task-server.json` (`schema_stamps`: service
//!   name → stamp), computed from the descriptors it actually
//!   mounts.
//! - **Clients** compute the same stamps from the descriptors
//!   they were compiled against and compare: a mismatch means
//!   "one of us is stale — rebuild" (`task doctor` does this for
//!   the CLI; ui-lab's smoke does it for the TS bundle, where
//!   the stamp folds the generated descriptor's method ids with
//!   the same XOR + 16-hex-digit format).
//!
//! XOR keeps the fold order-independent and trivially mirrored
//! in TypeScript (`BigInt` xor over the generated method ids) —
//! it's a *dev tripwire*, not a cryptographic identity.

use vox_types::ServiceDescriptor;

/// Stamp one service: XOR of its method ids, formatted as 16
/// lowercase hex digits (no `0x` prefix).
#[must_use]
pub fn service_stamp(descriptor: &ServiceDescriptor) -> String {
    let acc = descriptor.methods.iter().fold(0u64, |acc, m| acc ^ m.id.0);
    format!("{acc:016x}")
}

/// Stamp many services: `(service name, stamp)` pairs in input
/// order. The server serializes this into the well-known
/// document; clients diff their own copy against it by name.
#[must_use]
pub fn stamp_services<'a>(
    descriptors: impl IntoIterator<Item = &'a ServiceDescriptor>,
) -> Vec<(&'a str, String)> {
    descriptors
        .into_iter()
        .map(|d| (d.service_name, service_stamp(d)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_is_stable_and_shape_sensitive() {
        // Use this crate's own service descriptor as the fixture.
        let d = crate::org_management_descriptor();
        let a = service_stamp(d);
        let b = service_stamp(d);
        assert_eq!(a, b, "stamp is deterministic");
        assert_eq!(a.len(), 16, "16 hex digits");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        // Folding one extra (fake) method id flips the stamp —
        // the property the guard relies on.
        let with_extra = format!(
            "{:016x}",
            d.methods.iter().fold(0xdead_beefu64, |acc, m| acc ^ m.id.0)
        );
        assert_ne!(a, with_extra);
    }
}
