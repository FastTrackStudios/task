//! An account's devices, introduced to each other by the issuer.
//!
//! A person signed in with one FastTrackStudio account on a phone and a
//! laptop expects their work on both. Each device runs its own node
//! ([`LocalNode`]), and nodes sync directly over iroh once each admits
//! the other — but somebody has to tell them about each other. The
//! issuer (`fts-auth`) keeps the account's device address book
//! (`architect_auth::DeviceService`): which org the account's devices
//! share, and each device's endpoint id for it. [`sync_account`] is one
//! pass of a device's side of that:
//!
//! 1. register this install, offering its own personal org as the
//!    account's if the account has none — the first device's org is the
//!    one they all share;
//! 2. if this device does not hold the shared org yet, join it from
//!    another device that does, under the account's slug or, when that
//!    is taken here (every device provisions `personal`), one of its own
//!    — the slug is local; peers dial an org's endpoint, not its name;
//! 3. say this device's endpoint id for it;
//! 4. admit every other device's endpoint, so each may dial the other.
//!
//! Every step is idempotent, so the app runs it on every launch and
//! every few minutes after: the first device learns of the second the
//! next time it passes.
//!
//! What the device held before stays on it: a second device's own
//! personal org is not merged into the shared one.
// t[impl offline.account]

use std::path::Path;

use architect_auth::{AccountOrg, DeviceRegistration, DeviceService, PeerDevice};
use eyre::{WrapErr, bail};
use tracing::info;
use uuid::Uuid;

use crate::local_node::{JoinSpec, LocalNode, PERSONAL_SLUG};

/// The install's own id, beside `orgs/`: made once, kept with the data.
pub const INSTALL_FILE: &str = "install-id";

/// The install's id, made on first use.
pub fn install_id(data_root: &Path) -> eyre::Result<Uuid> {
    let path = data_root.join(INSTALL_FILE);
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(id) = text.trim().parse::<Uuid>() {
            return Ok(id);
        }
    }
    let id = Uuid::new_v4();
    std::fs::create_dir_all(data_root).wrap_err("create the data root")?;
    std::fs::write(&path, format!("{id}\n"))
        .wrap_err_with(|| format!("write {}", path.display()))?;
    Ok(id)
}

/// The id this install registers under for `account` (the issuer's
/// subject). Per account, so an install signed in as somebody else is a
/// different device to the issuer — which refuses an id another account
/// holds.
#[must_use]
pub fn device_id(install: Uuid, account: &str) -> Uuid {
    Uuid::new_v5(&install, account.as_bytes())
}

/// Who this device is, as it tells the issuer.
#[derive(Clone, Debug)]
pub struct ThisDevice {
    pub device_id: Uuid,
    pub name: String,
    pub platform: String,
}

/// What a pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountSync {
    /// The shared org's local slug, once this device holds it.
    pub slug: Option<String>,
    /// Whether this pass joined it.
    pub joined: bool,
    /// The other devices' endpoints now admitted here.
    pub admitted: Vec<String>,
    /// The account's other devices.
    pub others: Vec<PeerDevice>,
}

/// The org this node would offer as the account's: its own personal org,
/// never the planted example (a debug build's demo studio is not
/// anybody's work).
fn offer(node: &LocalNode) -> Option<AccountOrg> {
    let slug = node.owner().org.clone();
    if slug != PERSONAL_SLUG {
        return None;
    }
    let manifest = node.manifest(&slug).ok()?;
    Some(AccountOrg {
        id: manifest.id,
        slug: manifest.slug,
        display_name: manifest.display_name,
    })
}

/// A slug for the shared org here: the account's, unless this node
/// already holds a different org by that name.
fn local_slug(node: &LocalNode, org: &AccountOrg) -> String {
    if !node.hosts(&org.slug) {
        return org.slug.clone();
    }
    let short: String = org.id.simple().to_string().chars().take(6).collect();
    format!("{}-{short}", org.slug)
}

/// One pass of this device's side of the account's address book, with
/// `token` (the account's session or access token) against `issuer`.
pub async fn sync_account(
    node: &LocalNode,
    issuer: &impl DeviceService,
    token: &str,
    me: &ThisDevice,
) -> eyre::Result<AccountSync> {
    let register =
        |endpoint_id: Option<String>, offer_org: Option<AccountOrg>| DeviceRegistration {
            device_id: me.device_id,
            name: me.name.clone(),
            platform: me.platform.clone(),
            endpoint_id,
            offer_org,
        };
    let held_endpoint = |dir: &architect_auth::DeviceDirectory| {
        dir.account_org
            .as_ref()
            .and_then(|o| node.slug_of(o.id))
            .and_then(|s| node.endpoint_id(&s))
    };

    // 1. Here I am — and here is an org, if the account has none.
    let first = issuer
        .register_device(token.to_owned(), register(None, offer(node)))
        .await
        .map_err(|e| eyre::eyre!("register with the issuer: {e:?}"))?;
    let Some(org) = first.account_org.clone() else {
        // Nothing to share yet: this node offered nothing (an example
        // studio) and no other device has.
        return Ok(AccountSync {
            others: others(&first.devices, me.device_id),
            ..AccountSync::default()
        });
    };

    // 2. Hold it.
    let mut joined = false;
    let slug = match node.slug_of(org.id) {
        Some(slug) => slug,
        None => {
            let Some(peer) = first
                .devices
                .iter()
                .filter(|d| d.device_id != me.device_id)
                .find_map(|d| d.endpoint_id.clone())
            else {
                // The device that set it has not said where it is yet.
                return Ok(AccountSync {
                    others: others(&first.devices, me.device_id),
                    ..AccountSync::default()
                });
            };
            let slug = local_slug(node, &org);
            node.join(JoinSpec {
                slug: slug.clone(),
                id: org.id,
                display_name: org.display_name.clone(),
                peer: Some(peer),
                role: Some("admin".to_owned()),
            })
            .await
            .wrap_err_with(|| format!("join the account's org `{}`", org.slug))?;
            info!(%slug, org = %org.id, "account: joined the org this account's devices share");
            joined = true;
            slug
        }
    };
    let endpoint = match node.endpoint_id(&slug) {
        Some(e) => Some(e),
        None => node.bind(&slug).await,
    };
    if endpoint.is_none() {
        bail!("`{slug}` has no endpoint here (peers are off)");
    }

    // 3. Where I am, if the issuer does not know it yet.
    let dir = if held_endpoint(&first).is_some() && mine(&first, me.device_id) == endpoint {
        first
    } else {
        issuer
            .register_device(token.to_owned(), register(endpoint, None))
            .await
            .map_err(|e| eyre::eyre!("tell the issuer this device's endpoint: {e:?}"))?
    };

    // 4. Let the others in.
    let others = others(&dir.devices, me.device_id);
    let mut admitted = Vec::new();
    for peer in others.iter().filter_map(|d| d.endpoint_id.as_deref()) {
        node.admit_peer(&slug, peer)?;
        admitted.push(peer.to_owned());
    }
    Ok(AccountSync {
        slug: Some(slug),
        joined,
        admitted,
        others,
    })
}

fn others(devices: &[PeerDevice], me: Uuid) -> Vec<PeerDevice> {
    devices
        .iter()
        .filter(|d| d.device_id != me)
        .cloned()
        .collect()
}

fn mine(dir: &architect_auth::DeviceDirectory, me: Uuid) -> Option<String> {
    dir.devices
        .iter()
        .find(|d| d.device_id == me)
        .and_then(|d| d.endpoint_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_id_is_per_install_and_per_account() {
        let install = Uuid::new_v4();
        assert_eq!(device_id(install, "a"), device_id(install, "a"));
        assert_ne!(device_id(install, "a"), device_id(install, "b"));
        assert_ne!(device_id(install, "a"), device_id(Uuid::new_v4(), "a"));
    }

    #[test]
    fn the_install_id_is_made_once() {
        let dir = tempfile::tempdir().unwrap();
        let a = install_id(dir.path()).unwrap();
        assert_eq!(install_id(dir.path()).unwrap(), a);
    }
}
