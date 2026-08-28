//! Which servers may host this org — `files.peering.*`.
//!
//! A person authenticates with a session token; a receiver on another
//! server authenticates with an offer secret; a **host** authenticates
//! with the identity it already has. An iroh connection is mutually
//! authenticated by construction, so the remote endpoint id arriving on
//! a connection is a verified public key rather than a claim. There is
//! no credential to mint, store, rotate or leak — the credential is the
//! endpoint you are already dialling by.
//!
//! What was missing is a record of which of those identities this org
//! admits. [`files_domain::Peering`] modelled it exactly and nothing
//! wrote it down; this is the writing down, per org, durable.
//!
//! # Why the host set is not the grant table
//!
//! A grant says what a *person* may reach inside a root. Admission says
//! which *machines* run this org. They differ in what they protect
//! against — a wrong grant leaks a subtree to a colleague, a wrong
//! admission hands an org's whole commit graph to a stranger — and in
//! how they are revoked. Keeping them apart is also what stops
//! admitting a server from meaning "issue it a login", which is what
//! the peering chapter had to do before this existed.

use std::collections::BTreeMap;

use files_domain::{HostId, Hosting};

use crate::backend::FilesBackend;
use crate::durable::Scoped;

/// This org's admitted hosts, by endpoint id.
///
/// Only this org's, because the backend is per-org: `Peering` is keyed
/// by org because one *host* runs many, and from inside a single org's
/// backend the org is already decided.
#[derive(Debug, Default, Clone, facet::Facet)]
#[repr(C)]
pub struct Admitted(BTreeMap<HostId, Hosting>);

/// The on-disk shape: rows, with the id read back off each one.
#[derive(Default, facet::Facet)]
#[repr(C)]
pub struct AdmittedWire {
    hosts: Vec<HostRow>,
}

#[derive(facet::Facet)]
#[repr(C)]
struct HostRow {
    host: HostId,
    hosting: Hosting,
}

impl crate::durable::Durable for Admitted {
    type Wire = AdmittedWire;

    fn to_wire(&self) -> AdmittedWire {
        AdmittedWire {
            hosts: self
                .0
                .iter()
                .map(|(host, hosting)| HostRow {
                    host: host.clone(),
                    hosting: *hosting,
                })
                .collect(),
        }
    }

    fn from_wire(wire: AdmittedWire) -> Self {
        Self(
            wire.hosts
                .into_iter()
                .map(|r| (r.host, r.hosting))
                .collect(),
        )
    }
}

static ADMITTED: Scoped<Admitted> = Scoped::new("hosts");

impl FilesBackend {
    /// Admit a server to host this org.
    ///
    /// Idempotent, and re-admitting with a different [`Hosting`] is how
    /// a host's role changes — a structure-only host becoming a backup
    /// is not a new host.
    pub fn admit_host(&self, host: HostId, hosting: Hosting) {
        ADMITTED.write(self, |admitted| {
            admitted.0.insert(host, hosting);
        });
    }

    /// Stop admitting a server.
    ///
    /// Takes effect on that host's next call rather than on its
    /// cooperation — the same property a withdrawn offer has, and for
    /// the same reason: revocation that needs the other side to agree
    /// is not revocation.
    pub fn dismiss_host(&self, host: &HostId) {
        ADMITTED.write(self, |admitted| {
            admitted.0.remove(host);
        });
    }

    /// How this org is hosted by `host`, if at all.
    #[must_use]
    pub fn admits(&self, host: &HostId) -> Option<Hosting> {
        ADMITTED.read(self, |admitted| admitted.0.get(host).copied())
    }

    /// Every host this org admits, and how each holds it.
    #[must_use]
    pub fn admitted_hosts(&self) -> Vec<(HostId, Hosting)> {
        ADMITTED.read(self, |admitted| {
            admitted
                .0
                .iter()
                .map(|(h, k)| (h.clone(), *k))
                .collect::<Vec<_>>()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(dir: &std::path::Path) -> FilesBackend {
        FilesBackend::new(dir, dir.join("vault")).expect("backend")
    }

    fn host(id: &str) -> HostId {
        HostId(id.to_string())
    }

    #[test]
    fn an_unadmitted_host_is_not_admitted() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = backend(tmp.path());
        // The default has to be "no", not "no record therefore fine":
        // an empty file is the state every org starts in.
        assert_eq!(backend.admits(&host("stranger")), None);
    }

    #[test]
    fn admitting_is_idempotent_and_re_admitting_changes_the_role() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = backend(tmp.path());
        backend.admit_host(host("eu-west"), Hosting::structure_only());
        backend.admit_host(host("eu-west"), Hosting::structure_only());
        assert_eq!(backend.admitted_hosts().len(), 1);

        backend.admit_host(host("eu-west"), Hosting::backup());
        assert_eq!(
            backend.admitted_hosts().len(),
            1,
            "a role change is not a new host"
        );
        assert_eq!(backend.admits(&host("eu-west")), Some(Hosting::backup()));
    }

    #[test]
    fn dismissing_takes_a_host_off_the_list() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = backend(tmp.path());
        backend.admit_host(host("eu-west"), Hosting::working());
        backend.dismiss_host(&host("eu-west"));
        assert_eq!(backend.admits(&host("eu-west")), None);
    }

    /// Admission outlives the process, or a restart re-opens an org to
    /// everyone it had admitted and to nobody it had dismissed.
    #[test]
    fn admission_is_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let backend = backend(tmp.path());
            backend.admit_host(host("offsite"), Hosting::backup());
        }
        // `Scoped` caches by data dir within a process, so a second
        // backend alone would answer from memory having read nothing.
        let raw = std::fs::read_to_string(tmp.path().join("hosts.json"))
            .expect("the admitted set reached disk");
        assert!(raw.contains("offsite"), "{raw}");
    }

    /// One host, two orgs, and the answer differs — the property that
    /// makes this per-org rather than a process-wide set.
    #[test]
    fn admission_is_per_org() {
        let acme = tempfile::tempdir().unwrap();
        let vnt = tempfile::tempdir().unwrap();
        let (acme, vnt) = (backend(acme.path()), backend(vnt.path()));

        acme.admit_host(host("shared-machine"), Hosting::working());

        assert!(acme.admits(&host("shared-machine")).is_some());
        assert_eq!(
            vnt.admits(&host("shared-machine")),
            None,
            "one org's admission admitted a host to another"
        );
    }
}
