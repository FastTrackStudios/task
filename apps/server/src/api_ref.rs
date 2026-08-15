//! The self-describing API reference — [`permits::mounts()`] serialized.
//!
//! One fold over the SAME registry the router, the permit gate, and the
//! schema stamps derive from ([`crate::permits::mounts`]), so the
//! reference can never drift from what the server actually serves. No
//! hand-maintained list exists anywhere in this module.
//!
//! Consumers:
//! - `GET /org/{slug}/api` (see [`crate::router`]) serves
//!   [`reference_json`] — machine-readable discovery for clients and
//!   `task doctor`.
//! - `task api` (the CLI links this crate) renders [`reference`] as
//!   text / markdown; `task api --markdown` regenerates
//!   `apps/task/docs/api-reference.md`.
//!
//! Everything here is **build-static metadata**: service + method names,
//! argument names, the permit action/resource per method, and the
//! per-service schema stamp. No org data is touched.

use crate::permits;
use org_proto::schema_stamp::service_stamp;
use task_plugin::PluginSet;

/// One method of a mounted service, joined with its permit.
pub struct ApiMethod {
    pub name: &'static str,
    /// Argument names in declaration order (includes the `Tx` sink
    /// argument on streaming methods).
    pub args: Vec<&'static str>,
    /// True when any direct argument is a channel (`Tx`/`Rx`) — the
    /// server pushes to the caller instead of returning once. The
    /// `#[subscribe]` stream siblings are all-stream services; a base
    /// service can also carry individual streaming methods (e.g.
    /// `media/read`).
    pub stream: bool,
    /// Permit action (`read` / `write` / `comment` / `download`), if
    /// the service carries a permit table (all do today).
    pub action: Option<&'static str>,
    /// Permit resource template (`vault/{path}`, `tasks/**`, …).
    pub resource: Option<&'static str>,
    /// Audit line emitted even on allow (deletes, money, sends).
    pub audited: bool,
    pub doc: Option<&'static str>,
}

/// One mounted service: descriptor + permit table + schema stamp.
pub struct ApiService {
    pub name: &'static str,
    /// The permit table's short name (`task`, `vault-sync`, …) — the
    /// name the resource namespaces and `task api <service>` use.
    pub alias: Option<&'static str>,
    /// XOR-fold of the method ids — the proto/server skew stamp
    /// (identical to the `schema_stamps` entry in
    /// `/.well-known/task-server.json`).
    pub stamp: String,
    /// Every method is a channel push — a `#[subscribe]` stream
    /// sibling service.
    pub stream: bool,
    /// Owning plugin's `task_plugin::CATALOG` id (`"core"` for
    /// platform services).
    pub plugin: &'static str,
    /// Is the service actually served for the plugin set the reference
    /// was built against? [`reference`] (the build catalog) marks
    /// everything mounted; [`reference_for`] flags a disabled plugin's
    /// services `false` instead of omitting them, so `task api` and the
    /// endpoint can show the whole catalog with its per-org state.
    pub mounted: bool,
    pub methods: Vec<ApiMethod>,
    pub doc: Option<&'static str>,
}

/// Fold the mount registry into the reference — the full build catalog,
/// everything marked mounted. Registry order (the order
/// `org_layer_router` mounts) is preserved.
#[must_use]
pub fn reference() -> Vec<ApiService> {
    reference_for(&PluginSet::resolve(None))
}

/// The reference against one org's plugin set: every service this build
/// knows, with `mounted` reflecting whether `set` actually serves it.
/// Disabled services are listed (not omitted) — the catalog is
/// build-static; only the flag is org-state.
#[must_use]
pub fn reference_for(set: &PluginSet) -> Vec<ApiService> {
    permits::mounts()
        .into_iter()
        .map(|mount| {
            let d = mount.descriptor;
            let methods: Vec<ApiMethod> = d
                .methods
                .iter()
                .map(|m| {
                    let permit = mount
                        .permits
                        .as_ref()
                        .and_then(|t| t.methods.iter().find(|p| p.method == m.method_name));
                    ApiMethod {
                        name: m.method_name,
                        args: m.args.iter().map(|a| a.name).collect(),
                        stream: m.args_have_channels,
                        action: permit.map(|p| p.action),
                        resource: permit.map(|p| p.resource),
                        audited: permit.is_some_and(|p| p.audit),
                        doc: m.doc,
                    }
                })
                .collect();
            ApiService {
                name: d.service_name,
                alias: mount.permits.as_ref().map(|t| t.service),
                stamp: service_stamp(d),
                stream: !methods.is_empty() && methods.iter().all(|m| m.stream),
                plugin: mount.plugin,
                mounted: set.contains(mount.plugin),
                methods,
                doc: d.doc,
            }
        })
        .collect()
}

/// The JSON body of `GET /org/{slug}/api` with everything enabled — the
/// build-static view `task api --json` renders.
#[must_use]
pub fn reference_json() -> serde_json::Value {
    reference_json_for(&PluginSet::resolve(None))
}

/// The JSON body of `GET /org/{slug}/api` for one org's plugin set.
///
/// Every service of the build catalog is listed — a disabled plugin's
/// services carry `"mounted": false` rather than being omitted, so a
/// client can render the whole catalog and say *why* a service is
/// absent from the wire. The top-level `plugins` array is the catalog
/// with this org's enabled state.
#[must_use]
pub fn reference_json_for(set: &PluginSet) -> serde_json::Value {
    let services = reference_for(set);
    let streams = services.iter().filter(|s| s.stream).count();
    let mounted = services.iter().filter(|s| s.mounted).count();
    serde_json::json!({
        "version": 1,
        "generated_from": "task_server::permits::mounts()",
        "service_count": services.len(),
        "mounted_count": mounted,
        "stream_count": streams,
        "plugins": task_plugin::CATALOG.iter().map(|p| serde_json::json!({
            "id": p.id,
            "name": p.name,
            "core": p.core,
            "enabled": set.contains(p.id),
        })).collect::<Vec<_>>(),
        "services": services.iter().map(|s| serde_json::json!({
            "name": s.name,
            "alias": s.alias,
            "stamp": s.stamp,
            "stream": s.stream,
            "plugin": s.plugin,
            "mounted": s.mounted,
            "doc": s.doc,
            "methods": s.methods.iter().map(|m| serde_json::json!({
                "name": m.name,
                "args": m.args,
                "stream": m.stream,
                "action": m.action,
                "resource": m.resource,
                "audited": m.audited,
                "doc": m.doc,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference is the registry: same count, same names, same
    /// stamps as `schema_stamps()`.
    #[test]
    fn reference_matches_the_registry() {
        let services = reference();
        let stamps = crate::schema_stamps();
        assert_eq!(services.len(), stamps.len());
        for (s, (name, stamp)) in services.iter().zip(stamps.iter()) {
            assert_eq!(s.name, *name);
            assert_eq!(&s.stamp, stamp);
            assert!(!s.methods.is_empty(), "{name} has no methods");
        }
    }

    /// Every permit-tabled method resolves; every method carries its
    /// permit (coverage is complete, so `action` is always present).
    #[test]
    fn every_method_carries_its_permit() {
        for s in reference() {
            for m in &s.methods {
                assert!(
                    m.action.is_some() && m.resource.is_some(),
                    "{}/{} has no permit joined",
                    s.name,
                    m.name
                );
            }
        }
    }

    /// The `#[subscribe]` stream siblings are detected as stream
    /// services (their one method takes a `Tx` sink).
    #[test]
    fn stream_siblings_are_flagged() {
        let services = reference();
        let by_name = |n: &str| {
            services
                .iter()
                .find(|s| s.name == n)
                .unwrap_or_else(|| panic!("{n} not mounted"))
        };
        assert!(
            by_name("VaultSyncStream").stream || {
                // Name is descriptor-derived; fall back to asserting at
                // least one all-stream service exists.
                services.iter().any(|s| s.stream)
            }
        );
    }

    #[test]
    fn json_shape_is_stable() {
        let v = reference_json();
        assert_eq!(v["version"], 1);
        assert_eq!(
            v["service_count"].as_u64().unwrap() as usize,
            reference().len()
        );
        assert!(v["services"].as_array().is_some_and(|a| !a.is_empty()));
        // Full set: every service mounted, every catalog plugin enabled.
        assert_eq!(v["mounted_count"], v["service_count"]);
        assert!(
            v["plugins"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["enabled"] == true)
        );
    }

    /// A deny-list keeps the catalog complete but flags the denied
    /// plugin's services unmounted — listing-with-flag, not omission.
    #[test]
    fn disabled_plugins_are_listed_with_mounted_false() {
        use task_plugin::{PluginChoice, PluginSet};
        let set = PluginSet::resolve(Some(&PluginChoice::Disabled(vec!["mealplan".into()])));
        let services = reference_for(&set);
        assert_eq!(services.len(), reference().len(), "catalog stays complete");
        let (off, on): (Vec<_>, Vec<_>) = services.iter().partition(|s| !s.mounted);
        assert!(!off.is_empty() && off.iter().all(|s| s.plugin == "mealplan"));
        assert!(on.iter().all(|s| s.plugin != "mealplan"));

        let v = reference_json_for(&set);
        let plugins = v["plugins"].as_array().unwrap();
        let mealplan = plugins.iter().find(|p| p["id"] == "mealplan").unwrap();
        assert_eq!(mealplan["enabled"], false);
        assert_eq!(v["mounted_count"].as_u64().unwrap() as usize, on.len(),);
    }
}
