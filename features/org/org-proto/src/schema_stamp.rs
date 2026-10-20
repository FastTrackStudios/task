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
//!   the CLI; a TS client folds the generated descriptor's method
//!   ids with the same XOR + 16-hex-digit format).
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

/// Stamp many services: `(service name, stamp)` pairs, one per
/// **distinct** service name, in order of first appearance. The
/// server serializes this into the well-known document (a JSON map
/// keyed by name); clients diff their own copy against it by name.
///
/// Two descriptors may carry the same `service_name`: both
/// `#[vox::service]` and `#[architect::rpc]` take the wire name from
/// the trait's bare identifier, so `files_proto::SyncService` (the
/// facets/devices lane) and `files_sync::SyncService` (the replica
/// lane) are both `"SyncService"` on the wire. Their methods share one
/// id namespace (`sync-service.<method>`), so they are one wire
/// service and get ONE stamp — the XOR over every method of every
/// same-named descriptor.
///
/// Emitting one pair per descriptor instead (as this did) put the name
/// in the list twice. The server's JSON map kept the last stamp, the
/// client compared both, and `task doctor` reported `STALE` on
/// `SyncService` against a server built from the very same commit.
#[must_use]
pub fn stamp_services<'a>(
    descriptors: impl IntoIterator<Item = &'a ServiceDescriptor>,
) -> Vec<(&'a str, String)> {
    let mut folded: Vec<(&'a str, u64)> = Vec::new();
    for d in descriptors {
        let acc = d.methods.iter().fold(0u64, |acc, m| acc ^ m.id.0);
        match folded.iter_mut().find(|(name, _)| *name == d.service_name) {
            Some((_, stamp)) => *stamp ^= acc,
            None => folded.push((d.service_name, acc)),
        }
    }
    folded
        .into_iter()
        .map(|(name, acc)| (name, format!("{acc:016x}")))
        .collect()
}

/// What a client's stamps say about a server's.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Skew<'a> {
    /// Services both sides stamp, with different stamps — one side
    /// predates a proto change.
    pub stale: Vec<&'a str>,
    /// Services this build stamps that the server does not serve.
    pub unserved: Vec<&'a str>,
}

impl Skew<'_> {
    /// Every local stamp matched a served one.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.stale.is_empty() && self.unserved.is_empty()
    }
}

/// Diff this build's stamps (from [`stamp_services`]) against a
/// server's, looked up by name. The one comparison `task doctor` makes
/// and the integration suite pins, so the two cannot disagree about
/// what counts as skew.
#[must_use]
pub fn diff<'a, 's>(
    local: &'a [(&'a str, String)],
    served: impl Fn(&str) -> Option<&'s str>,
) -> Skew<'a> {
    let mut skew = Skew::default();
    for (name, stamp) in local {
        match served(name) {
            Some(s) if s == stamp => {}
            Some(_) => skew.stale.push(name),
            None => skew.unserved.push(name),
        }
    }
    skew
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

    /// Same-named descriptors are one wire service: one entry, XOR of
    /// both, at the first one's position — so a name-keyed map of the
    /// list (the well-known document) loses nothing.
    #[test]
    fn same_named_descriptors_fold_into_one_stamp() {
        use vox_types::{MethodDescriptor, MethodId};
        fn method(id: u64) -> MethodDescriptor {
            // Not `Clone`; copy the fixture's fields by hand.
            let d = crate::org_management_descriptor().methods[0];
            MethodDescriptor {
                id: MethodId(id),
                service_name: d.service_name,
                method_name: d.method_name,
                args_shape: d.args_shape,
                args: d.args,
                return_shape: d.return_shape,
                response_wire_shape: d.response_wire_shape,
                args_have_channels: d.args_have_channels,
                doc: d.doc,
            }
        }
        fn service(name: &'static str, ids: &[u64]) -> &'static ServiceDescriptor {
            let methods: Vec<&'static MethodDescriptor> = ids
                .iter()
                .map(|&id| &*Box::leak(Box::new(method(id))))
                .collect();
            Box::leak(Box::new(ServiceDescriptor {
                service_name: name,
                methods: Box::leak(methods.into_boxed_slice()),
                doc: None,
            }))
        }
        let a = service("SyncService", &[0x1, 0x2]);
        let other = service("Other", &[0x10]);
        let b = service("SyncService", &[0x4]);

        let stamps = stamp_services([a, other, b]);
        assert_eq!(
            stamps,
            vec![
                ("SyncService", format!("{:016x}", 0x1 ^ 0x2 ^ 0x4)),
                ("Other", format!("{:016x}", 0x10)),
            ]
        );

        // The served map holds exactly that, so the diff is clean —
        // and a server holding only one half's stamp is stale.
        let served: std::collections::HashMap<&str, String> =
            stamps.iter().map(|(n, s)| (*n, s.clone())).collect();
        assert!(diff(&stamps, |n| served.get(n).map(String::as_str)).is_clean());
        let half = format!("{:016x}", 0x4);
        let skew = diff(&stamps, |n| {
            if n == "SyncService" {
                Some(half.as_str())
            } else {
                served.get(n).map(String::as_str)
            }
        });
        assert_eq!(skew.stale, vec!["SyncService"]);
        assert!(skew.unserved.is_empty());
    }
}
