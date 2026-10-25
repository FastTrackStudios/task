//! This device among the account's others.
//!
//! Signed in with a FastTrackStudio account on a device that runs its
//! own node, the app keeps the account's device address book on the
//! issuer current: a pass of `task_server::account_devices` (through
//! [`task_ui_core::node::NodeHost::account_sync`]) on sign-in and every
//! few minutes after. The first pass of the first device makes its
//! personal org the one every device of the account shares; a later
//! device joins it; each admits the others, and the nodes sync directly
//! from then on. The issuer only introduces them.
//!
//! The last report is kept in a signal the Account page reads (the
//! account's other devices, and whether this one holds the shared org).
//! Failures are logged and retried on the next pass — a phone with no
//! signal is the ordinary case.

use dioxus::prelude::*;
use task_ui_core::node::AccountReport;

/// The last pass's report: `None` before one has run (or with no node /
/// no FastTrackStudio account).
#[derive(Clone, Copy)]
pub struct AccountDevices(pub Signal<Option<AccountReport>>);

/// Bumped when a pass changes what this device holds or lets the account
/// into — the org list re-discovers on it (`app.rs`).
#[derive(Clone, Copy)]
pub struct OrgsEpoch(pub Signal<u32>);

/// How often a signed-in device refreshes its place in the address book
/// and learns of new devices.
#[cfg(not(target_arch = "wasm32"))]
const EVERY_SECS: u64 = 5 * 60;

/// Whether a token is one the issuer minted (an OAuth access token, a
/// JWT) rather than a session on some server's own accounts — the only
/// kind worth showing the issuer.
#[must_use]
pub fn is_issuer_token(token: &str) -> bool {
    token.split('.').filter(|p| !p.is_empty()).count() == 3
}

/// What this device calls itself to the account's others: the name every
/// FTS app on it gives (`auth_client::this_device::name`: the Mac's
/// computer name, the host name, the iPad's or iPhone's name).
#[must_use]
pub fn device_name(person: &str) -> String {
    #[cfg(not(target_arch = "wasm32"))]
    let name = auth_client::this_device::name();
    #[cfg(target_arch = "wasm32")]
    let name = "Browser".to_owned();
    with_owner(person, &name)
}

/// iOS tells an app without the user-assigned-device-name entitlement
/// only the model ("iPhone", "iPad"): say whose it is.
fn with_owner(person: &str, name: &str) -> String {
    let first = person.split_whitespace().next().unwrap_or("").trim();
    let bare_model = matches!(name, "iPhone" | "iPad" | "iPod touch");
    if bare_model && !first.is_empty() {
        format!("{first}'s {name}")
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod name_tests {
    use super::with_owner;

    #[test]
    fn a_bare_model_says_whose_and_a_real_name_stands() {
        assert_eq!(with_owner("Cody Wright", "iPad"), "Cody's iPad");
        assert_eq!(with_owner("Cody", "iPhone"), "Cody's iPhone");
        assert_eq!(with_owner("", "iPad"), "iPad");
        assert_eq!(with_owner("Cody", "Cody's Mac mini"), "Cody's Mac mini");
    }
}

/// `ios`, `macos`, … as the issuer records it.
#[must_use]
pub fn platform() -> &'static str {
    if cfg!(target_os = "ios") {
        "ios"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

/// Mount the address-book pass. Call from `App`, after `provide_auth`.
pub fn use_account_devices() {
    let report = use_context_provider(|| AccountDevices(Signal::new(None)));
    let account = use_context::<Signal<Option<crate::auth::ActiveAccount>>>();
    let epoch = try_use_context::<OrgsEpoch>();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut report = report.0;
        use_effect(move || {
            let Some(a) = account.read().clone() else {
                report.set(None);
                return;
            };
            if !is_issuer_token(&a.token) || task_ui_core::node::host().is_none() {
                report.set(None);
                return;
            }
            spawn(async move {
                loop {
                    match pass(&a).await {
                        Ok(r) => {
                            tracing::info!(
                                slug = ?r.slug,
                                joined = r.joined,
                                others = r.others.len(),
                                "account: device address book refreshed"
                            );
                            if report.peek().as_ref() != Some(&r) {
                                if let Some(OrgsEpoch(mut e)) = epoch {
                                    e += 1;
                                }
                            }
                            report.set(Some(r));
                        }
                        Err(e) => tracing::warn!(%e, "account: device address book not reached"),
                    }
                    architect::sleep(architect::Duration::from_secs(EVERY_SECS)).await;
                    // Signed out, or switched, since: this loop is done.
                    if account.peek().as_ref().map(|x| &x.token) != Some(&a.token) {
                        return;
                    }
                }
            });
        });
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (report, account, epoch);
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn pass(a: &crate::auth::ActiveAccount) -> Result<AccountReport, String> {
    let node = task_ui_core::node::host().ok_or("this build holds no node")?;
    let issuer = task_ui_core::central_auth::issuer_vox()
        .or_else(|| {
            task_ui_core::central_auth::issuer_or_default().map(|base| {
                let ws = base
                    .replacen("https://", "wss://", 1)
                    .replacen("http://", "ws://", 1);
                format!("{ws}/vox")
            })
        })
        .ok_or("no FastTrackStudio issuer configured")?;
    let lane = task_ui_core::vox_clients::issuer_caller_at(&issuer, None).await?;
    node.account_sync(task_ui_core::node::AccountSyncRequest {
        issuer: lane,
        token: a.token.clone(),
        account: a.user_id.to_string(),
        device_name: device_name(&a.name),
        platform: platform().to_owned(),
    })
    .await
}

/// The account's devices, on the Account page: this one, then the
/// others the issuer knows. Shown once a pass has run.
#[component]
pub fn AccountDevicesSection() -> Element {
    let Some(AccountDevices(report)) = try_use_context::<AccountDevices>() else {
        return rsx! {};
    };
    let Some(r) = report.read().clone() else {
        return rsx! {};
    };
    let here = if r.slug.is_some() {
        "This device · syncing"
    } else {
        "This device · waiting for your others"
    };
    rsx! {
        div { "data-testid": "account-devices",
            task_ui_core::sheet::SheetSection { label: "Your devices".to_owned() }
            task_ui_core::sheet::SheetGroup {
                DeviceRow { name: device_name(""), platform: platform().to_owned(), note: here.to_owned() }
                for d in r.others.iter() {
                    DeviceRow {
                        key: "{d.name}{d.last_seen}",
                        name: d.name.clone(),
                        platform: d.platform.clone(),
                        note: if d.reachable { "Syncing".to_owned() } else { "Waiting for it to sign in again".to_owned() },
                    }
                }
            }
        }
    }
}

#[component]
fn DeviceRow(name: String, platform: String, note: String) -> Element {
    use architect_ui::lucide_dioxus as i;
    rsx! {
        div { class: "flex min-h-[54px] items-center gap-3 px-4",
            span { class: "text-muted-foreground",
                if platform == "ios" {
                    i::Smartphone { size: 18 }
                } else {
                    i::Laptop { size: 18 }
                }
            }
            span { class: "flex min-w-0 flex-1 flex-col",
                span { class: "truncate text-[15px] text-foreground", "{name}" }
                span { class: "truncate text-[13px] text-muted-foreground", "{note}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_issuers_jwt_goes_to_the_issuer() {
        assert!(is_issuer_token("aaa.bbb.ccc"));
        assert!(!is_issuer_token("a-node-session-token"));
        assert!(!is_issuer_token("a..b"));
    }
}
