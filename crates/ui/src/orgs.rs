//! Org **discovery** — the `/.well-known/task-server.json` fetch.
//!
//! The selection model itself ([`OrgMeta`], [`OrgSelection`],
//! [`selected_slugs`] and friends) lives in [`task_ui_core::orgs`] so
//! feature UI crates can scope their own fetches without depending on
//! this shell; it is re-exported here, so every `crate::orgs::…` path
//! still resolves.
//!
//! Discovery stays here because it is the one platform-specific piece:
//! `window.fetch` on wasm, `reqwest` (rustls — works in the iOS
//! sandbox) on native, with a Sentry breadcrumb on failure.

pub use task_ui_core::orgs::*;

// ── discovery ───────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct WellKnown {
    orgs: Vec<RawOrg>,
    /// Where accounts come from, when not from this server. Absent on
    /// a self-hosted server and on every server predating central
    /// auth — both mean "sign in against the home org".
    #[serde(default)]
    central_auth: Option<String>,
    /// The account the bearer we sent resolved to — `None` when we sent
    /// none, or the server did not recognise it. Absent on servers
    /// predating the field.
    #[serde(default)]
    principal: Option<RawPrincipal>,
}

#[derive(serde::Deserialize)]
struct RawPrincipal {
    id: uuid::Uuid,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// What discovery said about the token it carried.
///
/// Boot restore reads this instead of validating the cached token a
/// second time: the server has already resolved it (against its org
/// stores or the issuer) to tag membership, so `whoami` over the lane
/// and a `/userinfo` round trip to the issuer would only repeat that
/// work — three network hops that were the bulk of a five-second reload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPrincipal {
    /// The bearer this answer is about. A principal is only trusted for
    /// the exact token that produced it.
    pub token: String,
    pub user_id: uuid::Uuid,
    pub email: Option<String>,
    pub name: Option<String>,
}

static DISCOVERED: std::sync::RwLock<Option<DiscoveredPrincipal>> = std::sync::RwLock::new(None);

/// The principal the last discovery resolved, if it was for `token`.
#[must_use]
pub fn discovered_principal_for(token: &str) -> Option<DiscoveredPrincipal> {
    DISCOVERED
        .read()
        .ok()
        .and_then(|d| d.clone())
        .filter(|d| d.token == token)
}

fn note_principal(bearer: Option<&str>, raw: Option<RawPrincipal>) {
    let fresh = match (bearer, raw) {
        (Some(token), Some(p)) => Some(DiscoveredPrincipal {
            token: token.to_owned(),
            user_id: p.id,
            email: p.email,
            name: p.name,
        }),
        _ => None,
    };
    if let Ok(mut d) = DISCOVERED.write() {
        *d = fresh;
    }
}

#[derive(serde::Deserialize)]
struct RawOrg {
    slug: String,
    display_name: String,
    is_home: bool,
    #[serde(default)]
    id: Option<uuid::Uuid>,
    /// Plugin deny-list from the org's manifest. Absent on servers
    /// predating the plugin toggle — everything on.
    #[serde(default)]
    disabled_plugins: Vec<String>,
    /// Whether the presented session validates against this org.
    /// Absent on servers predating #109 criterion 6, and null when
    /// discovery ran without a token — both mean "unknown", which the
    /// client treats as "show it".
    #[serde(default)]
    member: Option<bool>,
    /// The org's iroh endpoint id — how a native client dials it
    /// without a URL. Absent on older servers and before the first
    /// bind.
    #[serde(default)]
    iroh: Option<String>,
}

fn parse_orgs(body: &str, bearer: Option<&str>) -> Result<Vec<OrgMeta>, String> {
    let wk: WellKnown = serde_json::from_str(body).map_err(|e| format!("parse well-known: {e}"))?;
    note_principal(bearer, wk.principal);
    let list: Vec<OrgMeta> = wk
        .orgs
        .into_iter()
        .map(|o| OrgMeta {
            slug: o.slug,
            name: o.display_name,
            is_home: o.is_home,
            id: o.id,
            disabled_plugins: o.disabled_plugins,
            member: o.member,
            iroh: o.iroh,
        })
        .collect();
    // Discovery is also where the client learns whether this server
    // issues its own accounts. Same reason as the endpoint ids below:
    // sign-in needs it from a plain async fn, not a component.
    task_ui_core::central_auth::note(wk.central_auth);
    // Discovery is where a native client learns each org's iroh
    // endpoint id; the transport keeps its own registry because
    // `caller_for` is a free fn with no reach into the org-list signal.
    #[cfg(not(target_arch = "wasm32"))]
    task_ui_core::iroh_transport::note_org_endpoints(
        list.iter().map(|o| (o.slug.as_str(), o.iroh.as_deref())),
    );
    Ok(list)
}

/// Fetch the hosted org list from `/.well-known/task-server.json`.
#[cfg(target_arch = "wasm32")]
async fn fetch_orgs_live() -> Result<Vec<OrgMeta>, String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let base = http_base();
    if base.is_empty() {
        return Err("no server URL configured".to_owned());
    }
    let url = format!("{base}/.well-known/task-server.json");
    let win = web_sys::window().ok_or("no window")?;
    // Discovery carries the session so the server can tag which orgs are
    // ours (#109 criterion 6). A bare `fetch_with_str` cannot set headers,
    // so build a Request. Unlike `<audio>`/`<img>`, `fetch` CAN send an
    // Authorization header — this is only about the API shape.
    let bearer = crate::vox_session::bearer();
    let resp_val = match &bearer {
        Some(token) => {
            let headers = web_sys::Headers::new().map_err(|e| format!("headers: {e:?}"))?;
            headers
                .set("authorization", &format!("Bearer {token}"))
                .map_err(|e| format!("set authorization: {e:?}"))?;
            let init = web_sys::RequestInit::new();
            init.set_headers(&headers);
            let req = web_sys::Request::new_with_str_and_init(&url, &init)
                .map_err(|e| format!("orgs request: {e:?}"))?;
            JsFuture::from(win.fetch_with_request(&req)).await
        }
        None => JsFuture::from(win.fetch_with_str(&url)).await,
    }
    .map_err(|e| format!("fetch orgs: {e:?}"))?;
    let resp: web_sys::Response = resp_val
        .dyn_into()
        .map_err(|_| "fetch returned a non-Response".to_owned())?;
    let text_promise = resp.text().map_err(|e| format!("orgs body: {e:?}"))?;
    let text_val = JsFuture::from(text_promise)
        .await
        .map_err(|e| format!("orgs body await: {e:?}"))?;
    let text = text_val.as_string().ok_or("orgs body not a string")?;
    parse_orgs(&text, bearer.as_deref())
}

/// Fetch the hosted org list from `/.well-known/task-server.json`.
///
/// Native (desktop/mobile, incl. iOS) has no `window.fetch`; use
/// `reqwest` (rustls — works in the iOS sandbox) over the same
/// [`http_base`]-derived URL. This is what makes an installed app with
/// no `TASK_VOX_URL` env connect: the user-selected server drives
/// [`http_base`], discovery resolves the org slug, and the vox dial can
/// proceed (`vox_clients::org_ws_url` needs a real slug).
#[cfg(not(target_arch = "wasm32"))]
async fn fetch_orgs_live() -> Result<Vec<OrgMeta>, String> {
    let base = http_base();
    if base.is_empty() {
        return Err("no server URL configured".to_owned());
    }
    let url = format!("{base}/.well-known/task-server.json");
    // Explicit connect + total timeouts: on device a stalled DNS/TLS/connect
    // would otherwise leave the resource pending forever ("org discovery
    // hasn't resolved yet" with no way to tell why). Fail fast + loud instead.
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("task-mobile")
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let result = async {
        let mut req = client.get(&url);
        let bearer = crate::vox_session::bearer();
        // Same tagging as wasm — native clients own their requests.
        if let Some(token) = &bearer {
            req = req.bearer_auth(token);
        }
        let body = req
            .send()
            .await
            .map_err(|e| format!("fetch orgs `{url}`: {e}"))?
            .error_for_status()
            .map_err(|e| format!("fetch orgs `{url}`: {e}"))?
            .text()
            .await
            .map_err(|e| format!("orgs body `{url}`: {e}"))?;
        parse_orgs(&body, bearer.as_deref())
    }
    .await;
    match &result {
        Ok(orgs) => tracing::info!(url, count = orgs.len(), "org discovery ok"),
        Err(e) => {
            tracing::warn!(url, error = %e, "org discovery failed");
            // Belt-and-suspenders: capture directly so this failure
            // reaches Sentry even if the client's tracing subscriber was
            // superseded by dioxus's own subscriber init.
            #[cfg(not(target_arch = "wasm32"))]
            sentry::capture_message(
                &format!("org discovery failed: {e}"),
                sentry::Level::Warning,
            );
        }
    }
    result
}

// ── discovery boot cache ────────────────────────────────────────────
//
// Discovery is a network fetch, and *everything* org-scoped is
// downstream of it: with no org list there is no slug, so no page can
// even name what it wants. That made every offline surface dead on
// arrival regardless of its own caching — the email page's offline
// cache could never be reached, because the page had no key to look
// under.
//
// So the last successful list is remembered per server base, and a
// failed discovery falls back to it. A live answer always wins and
// refreshes; the cache only ever covers the "server unreachable" case,
// never a server that answered with a different list.

fn orgs_cache_key(base: &str) -> String {
    // One entry per server — pointing the app at a different server
    // must not show you the previous one's orgs.
    let safe: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("task.orgs.{safe}")
}

#[cfg(target_arch = "wasm32")]
fn orgs_cache_read(key: &str) -> Option<String> {
    web_sys::window()?
        .local_storage()
        .ok()
        .flatten()?
        .get_item(key)
        .ok()
        .flatten()
}

#[cfg(target_arch = "wasm32")]
fn orgs_cache_write(key: &str, value: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(key, value);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn orgs_cache_path(key: &str) -> Option<std::path::PathBuf> {
    Some(dirs::cache_dir()?.join("task").join(key))
}

#[cfg(not(target_arch = "wasm32"))]
fn orgs_cache_read(key: &str) -> Option<String> {
    std::fs::read_to_string(orgs_cache_path(key)?).ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn orgs_cache_write(key: &str, value: &str) {
    let Some(path) = orgs_cache_path(key) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, value);
}

/// Discover the hosted orgs, falling back to the last known list when
/// the server cannot be reached.
pub async fn fetch_orgs() -> Result<Vec<OrgMeta>, String> {
    let base = http_base();
    match fetch_orgs_live().await {
        Ok(list) => {
            if !base.is_empty() {
                if let Ok(json) = serde_json::to_string(&list) {
                    orgs_cache_write(&orgs_cache_key(&base), &json);
                }
            }
            Ok(list)
        }
        Err(err) => {
            if base.is_empty() {
                return Err(err);
            }
            match orgs_cache_read(&orgs_cache_key(&base))
                .and_then(|j| serde_json::from_str::<Vec<OrgMeta>>(&j).ok())
                .filter(|l| !l.is_empty())
            {
                Some(cached) => Ok(cached),
                None => Err(err),
            }
        }
    }
}

#[cfg(test)]
mod orgs_cache_tests {
    use super::orgs_cache_key;

    #[test]
    fn cache_keys_are_per_server() {
        assert_ne!(
            orgs_cache_key("https://tasks.starcommand.live"),
            orgs_cache_key("http://127.0.0.1:18080")
        );
        // No separators survive into the filename — the base is
        // user-supplied and lands in a path on desktop.
        let k = orgs_cache_key("https://a.b/../../etc");
        assert!(!k.contains('/') && !k.contains('.') || k.starts_with("task.orgs."));
        assert!(!k["task.orgs.".len()..].contains('/'));
    }
}
