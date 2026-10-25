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
/// The id where the platform gives none, and the record of the id an
/// older Task registered under (see [`previous_install_id`]).
pub const INSTALL_FILE: &str = "install-id";

/// This device's install id: the platform's, which every FastTrackStudio
/// app on the device derives alike (`auth_client::this_device`), so Task
/// and Studio on one iPad or Mac are one device to the issuer. Where the
/// platform gives none, the one made once in `<data root>/install-id`.
pub fn install_id(data_root: &Path) -> eyre::Result<Uuid> {
    match auth_client::this_device::install_id() {
        Some(id) => Ok(id),
        None => file_install_id(data_root),
    }
}

/// The install id kept in `<data root>/install-id`, made on first use:
/// one per data root. What a server (`task-server peer`) registers
/// under, where the machine's id may be one a container image shares.
pub fn file_install_id(data_root: &Path) -> eyre::Result<Uuid> {
    if let Some(id) = read_install_file(data_root) {
        return Ok(id);
    }
    let id = Uuid::new_v4();
    write_install_file(data_root, id)?;
    Ok(id)
}

/// The install id an older Task registered under, when it is not
/// `current`: the random one it kept in the file. Its device row is
/// retired once ([`ThisDevice::retire`]), then [`settle_install_id`]
/// rewrites the file.
#[must_use]
pub fn previous_install_id(data_root: &Path, current: Uuid) -> Option<Uuid> {
    read_install_file(data_root).filter(|id| *id != current)
}

/// The file says `current` from now on: its earlier id is retired.
pub fn settle_install_id(data_root: &Path, current: Uuid) -> eyre::Result<()> {
    write_install_file(data_root, current)
}

fn read_install_file(data_root: &Path) -> Option<Uuid> {
    std::fs::read_to_string(data_root.join(INSTALL_FILE))
        .ok()
        .and_then(|text| text.trim().parse::<Uuid>().ok())
}

fn write_install_file(data_root: &Path, id: Uuid) -> eyre::Result<()> {
    let path = data_root.join(INSTALL_FILE);
    std::fs::create_dir_all(data_root).wrap_err("create the data root")?;
    std::fs::write(&path, format!("{id}\n")).wrap_err_with(|| format!("write {}", path.display()))
}

/// The id this install registers under for `account` (the issuer's
/// subject). Per account, so an install signed in as somebody else is a
/// different device to the issuer — which refuses an id another account
/// holds. The same derivation as `auth_client::this_device::device_id`,
/// which Studio registers under.
#[must_use]
pub fn device_id(install: Uuid, account: &str) -> Uuid {
    auth_client::this_device::device_id(install, account)
}

/// Who this device is, as it tells the issuer.
#[derive(Clone, Debug)]
pub struct ThisDevice {
    pub device_id: Uuid,
    pub name: String,
    pub platform: String,
    /// The account (the issuer's subject) signed in here — let into this
    /// device's own personal org and the shared one, since a device
    /// signed in as somebody is that person's device.
    pub account: Uuid,
    /// The id this device was registered under before it took the
    /// platform's install id (an older Task's random one): its row is
    /// taken off the account by the pass, so the device is listed once.
    pub retire: Option<Uuid>,
}

/// What a pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountSync {
    /// The [`ThisDevice::retire`] row is gone from the account (taken off
    /// by this pass, or never there): the caller can settle the install
    /// id ([`settle_install_id`]).
    pub retired: bool,
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
            // Task's nodes dial each other by endpoint id with address
            // lookup; they do not pin a relay.
            relay_url: None,
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
    let stale = stale_rows(me, &first.devices);
    let retired = retire(issuer, token, me, &first).await;
    // The other devices: never this one under an old row.
    let others = |devices: &[PeerDevice]| -> Vec<PeerDevice> {
        devices
            .iter()
            .filter(|d| d.device_id != me.device_id && !stale.contains(&d.device_id))
            .cloned()
            .collect()
    };
    // This device's own personal org is the signed-in person's, whatever
    // the account shares (never the demo: its owner is not `personal`).
    if node.owner().org == PERSONAL_SLUG {
        node.admit_account(me.account, &[PERSONAL_SLUG.to_owned()], "admin")
            .await
            .wrap_err("let the account into this device's org")?;
    }
    let Some(org) = first.account_org.clone() else {
        // Nothing to share yet: this node offered nothing (an example
        // studio) and no other device has.
        return Ok(AccountSync {
            retired,
            others: others(&first.devices),
            ..AccountSync::default()
        });
    };

    // 2. Hold it.
    let mut joined = false;
    let slug = match node.slug_of(org.id) {
        Some(slug) => slug,
        None => {
            let Some(peer) = others(&first.devices)
                .into_iter()
                .find_map(|d| d.endpoint_id)
            else {
                // The device that set it has not said where it is yet.
                return Ok(AccountSync {
                    retired,
                    others: others(&first.devices),
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

    // 4. The signed-in account is let into the shared org here.
    node.admit_account(me.account, std::slice::from_ref(&slug), "admin")
        .await
        .wrap_err("let the account into its orgs here")?;

    // 5. Let the others in.
    let others = others(&dir.devices);
    let mut admitted = Vec::new();
    for peer in others.iter().filter_map(|d| d.endpoint_id.as_deref()) {
        node.admit_peer(&slug, peer)?;
        admitted.push(peer.to_owned());
    }
    Ok(AccountSync {
        retired,
        slug: Some(slug),
        joined,
        admitted,
        others,
    })
}

/// The account's other rows that are this device: [`ThisDevice::retire`]
/// (the id an older Task registered under), and any row with this
/// device's name and platform under another id (made by an app before
/// every app took its id from `auth_client::this_device`, or by a
/// reinstall that lost its install-id file). The same name-and-platform
/// rule `DeviceRegistration::is_listed` uses to tell an app the device is
/// already listed.
fn stale_rows(me: &ThisDevice, devices: &[PeerDevice]) -> Vec<Uuid> {
    let name = me.name.trim();
    devices
        .iter()
        .filter(|d| d.device_id != me.device_id)
        .filter(|d| {
            Some(d.device_id) == me.retire
                || (d.platform == me.platform && d.name.trim().eq_ignore_ascii_case(name))
        })
        .map(|d| d.device_id)
        .collect()
}

/// Take this device's [`stale_rows`] off the account, once it holds its
/// own row. `true` when none is left: taken off now, or none there (a
/// pass after an earlier one whose file write was lost finds nothing).
/// A refusal is logged and retried on the next pass: it never fails one.
async fn retire(
    issuer: &impl DeviceService,
    token: &str,
    me: &ThisDevice,
    dir: &architect_auth::DeviceDirectory,
) -> bool {
    let mut all_gone = true;
    for old in stale_rows(me, &dir.devices) {
        match issuer.forget_device(token.to_owned(), old).await {
            Ok(()) => {
                info!(device = %me.device_id, retired = %old, "account: a stale row of this device is off the account");
            }
            Err(e) => {
                tracing::warn!(retired = %old, error = ?e, "account: a stale row of this device is still listed");
                all_gone = false;
            }
        }
    }
    all_gone
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
        let f = file_install_id(dir.path()).unwrap();
        assert_eq!(file_install_id(dir.path()).unwrap(), f, "the file's, kept");
    }

    #[test]
    fn the_install_id_is_the_platforms_where_there_is_one() {
        // What every FTS app on this machine derives: Studio's too.
        if let Some(platform) = auth_client::this_device::install_id() {
            let dir = tempfile::tempdir().unwrap();
            assert_eq!(install_id(dir.path()).unwrap(), platform);
        }
    }

    fn peer(id: Uuid, name: &str, platform: &str) -> PeerDevice {
        PeerDevice {
            device_id: id,
            name: name.into(),
            platform: platform.into(),
            endpoint_id: None,
            relay_url: None,
            last_seen_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn this_devices_old_rows_are_its_old_id_and_its_name_on_another_id() {
        let (me, old, stray, laptop, other_phone) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        let this = ThisDevice {
            device_id: me,
            name: "Cody's iPhone".into(),
            platform: "ios".into(),
            account: Uuid::new_v4(),
            retire: Some(old),
        };
        let listed = [
            peer(me, "Cody's iPhone", "ios"),
            peer(old, "iPhone", "ios"),
            peer(stray, " cody's iphone ", "ios"),
            peer(laptop, "airlock", "macos"),
            peer(other_phone, "Cody's iPhone", "android"),
        ];
        let mut stale = stale_rows(&this, &listed);
        stale.sort();
        let mut want = vec![old, stray];
        want.sort();
        assert_eq!(stale, want, "its old id, and its name on this platform");
    }

    #[test]
    fn an_older_random_id_is_named_once_then_settled() {
        let dir = tempfile::tempdir().unwrap();
        let current = Uuid::new_v4();
        assert_eq!(previous_install_id(dir.path(), current), None, "no file");
        let old = file_install_id(dir.path()).unwrap();
        assert_eq!(previous_install_id(dir.path(), current), Some(old));
        settle_install_id(dir.path(), current).unwrap();
        assert_eq!(previous_install_id(dir.path(), current), None, "settled");
    }
}
