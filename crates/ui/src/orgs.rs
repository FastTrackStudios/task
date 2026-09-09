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

/// What one discovery answered, as it is cached.
///
/// The cache used to hold the org list alone, and that lost the issuer:
/// see [`fetch_orgs`], where a server that cannot be reached falls back
/// to this. An org list without its issuer is a client that has silently
/// forgotten it can offer single sign-on.
///
/// Serialised with the orgs under `orgs` rather than as a bare array, so
/// an entry written by an older build — which was that bare array — is
/// still readable. See [`Discovered::parse_cached`].
#[derive(serde::Serialize, serde::Deserialize)]
struct Discovered {
    orgs: Vec<OrgMeta>,
    /// The issuer this server advertised, if any. `None` is a real
    /// answer — a self-hosted server issues its own accounts.
    #[serde(default)]
    central_auth: Option<String>,
}

impl Discovered {
    /// Read a cache entry, accepting both shapes.
    ///
    /// The bare array is what every entry written before the issuer was
    /// cached looks like. Reading it as "orgs, no issuer" is exactly
    /// what those entries mean, and it keeps a stale cache from
    /// producing a parse error on the one path that exists to survive
    /// failure.
    fn parse_cached(json: &str) -> Option<Self> {
        if let Ok(full) = serde_json::from_str::<Self>(json) {
            return Some(full);
        }
        serde_json::from_str::<Vec<OrgMeta>>(json)
            .ok()
            .map(|orgs| Self {
                orgs,
                central_auth: None,
            })
    }

    /// Publish what this discovery said into the process-wide
    /// registries the sign-in path reads.
    ///
    /// Called on BOTH paths — live and cached — because the registries
    /// are what the UI gates on, and a value that reaches them only on
    /// the live path is a feature that disappears whenever the network
    /// hiccups.
    fn publish(&self) {
        task_ui_core::central_auth::note(self.central_auth.clone());
        // Discovery is where a native client learns each org's iroh
        // endpoint id; the transport keeps its own registry because
        // `caller_for` is a free fn with no reach into the org-list
        // signal.
        #[cfg(not(target_arch = "wasm32"))]
        task_ui_core::iroh_transport::note_org_endpoints(
            self.orgs
                .iter()
                .map(|o| (o.slug.as_str(), o.iroh.as_deref())),
        );
    }
}

fn parse_orgs(body: &str, bearer: Option<&str>) -> Result<Discovered, String> {
    let wk: WellKnown = serde_json::from_str(body).map_err(|e| format!("parse well-known: {e}"))?;
    note_principal(bearer, wk.principal);
    let orgs: Vec<OrgMeta> = wk
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
    // issues its own accounts. Sign-in needs it from a plain async fn,
    // not a component.
    let found = Discovered {
        orgs,
        central_auth: wk.central_auth,
    };
    found.publish();
    Ok(found)
}

/// Fetch the hosted org list from `/.well-known/task-server.json`.
#[cfg(target_arch = "wasm32")]
async fn fetch_orgs_live() -> Result<Discovered, String> {
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
async fn fetch_orgs_live() -> Result<Discovered, String> {
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
        Ok(found) => tracing::info!(url, count = found.orgs.len(), "org discovery ok"),
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

/// Discover the hosted orgs, falling back to the last known answer when
/// the server cannot be reached.
///
/// The fallback replays the whole answer, issuer included, not just the
/// list. It used to cache the orgs alone, and the cost was invisible:
/// the app looked fine — the org list was there — while
/// `central_auth::issuer()` stayed `None`, and the login screen gates
/// "Continue with FastTrackStudio" on exactly that. So a single failed
/// discovery removed single sign-on from the UI, silently, with no
/// error anywhere and no way for the person to tell that the button had
/// ever existed.
///
/// That is not hypothetical: it happened during a routine deploy, where
/// `/.well-known` answered 502 for a few seconds. Everyone who loaded
/// the page in that window got the password form and no way back to the
/// button short of a reload they had no reason to attempt.
pub async fn fetch_orgs() -> Result<Vec<OrgMeta>, String> {
    let base = http_base();
    match fetch_orgs_live().await {
        Ok(found) => {
            if !base.is_empty() {
                if let Ok(json) = serde_json::to_string(&found) {
                    orgs_cache_write(&orgs_cache_key(&base), &json);
                }
            }
            Ok(found.orgs)
        }
        Err(err) => {
            if base.is_empty() {
                return Err(err);
            }
            match orgs_cache_read(&orgs_cache_key(&base))
                .and_then(|j| Discovered::parse_cached(&j))
                .filter(|d| !d.orgs.is_empty())
            {
                Some(cached) => {
                    // The same registries the live path fills. Without
                    // this the client keeps the orgs and forgets the
                    // issuer, which is the bug this comment describes.
                    cached.publish();
                    Ok(cached.orgs)
                }
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

#[cfg(test)]
mod cached_discovery_tests {
    use super::Discovered;

    fn orgs_json() -> &'static str {
        r#"[{"slug":"codywright","name":"Cody Wright","is_home":true,
             "id":null,"disabled_plugins":[],"member":true,"iroh":null}]"#
    }

    /// The whole point: a cached answer still knows the issuer.
    ///
    /// Without it, one failed discovery takes "Continue with
    /// FastTrackStudio" off the login screen — the org list is still
    /// there, so nothing looks wrong, and the person is left with the
    /// password form and no way to tell the button ever existed.
    #[test]
    fn a_cached_answer_carries_the_issuer() {
        let json = format!(
            r#"{{"orgs":{},"central_auth":"https://auth.fasttrackstudio.app"}}"#,
            orgs_json()
        );
        let cached = Discovered::parse_cached(&json).expect("parses");
        assert_eq!(
            cached.central_auth.as_deref(),
            Some("https://auth.fasttrackstudio.app")
        );
        assert_eq!(cached.orgs.len(), 1);
    }

    /// A cache entry written by an older build is a BARE ARRAY. It has
    /// to keep parsing: this is the one path that exists to survive a
    /// server being unreachable, so a parse error here would turn a
    /// hiccup into a dead app on the first load after an upgrade.
    #[test]
    fn an_entry_from_before_the_issuer_was_cached_still_reads() {
        let cached = Discovered::parse_cached(orgs_json()).expect("the old shape still parses");
        assert_eq!(cached.orgs.len(), 1);
        assert_eq!(cached.central_auth, None, "it genuinely did not know one");
    }

    /// `None` is a real answer, not a missing one: a self-hosted server
    /// issues its own accounts, and the button must stay hidden there.
    #[test]
    fn a_self_hosted_server_caches_no_issuer() {
        let json = format!(r#"{{"orgs":{},"central_auth":null}}"#, orgs_json());
        let cached = Discovered::parse_cached(&json).expect("parses");
        assert_eq!(cached.central_auth, None);
    }

    /// Round trip: what the live path writes is what the cached path
    /// reads. These are the two halves that drifted apart before.
    #[test]
    fn what_is_written_is_what_is_read() {
        let found = Discovered {
            orgs: serde_json::from_str(orgs_json()).expect("orgs"),
            central_auth: Some("https://auth.fasttrackstudio.app".to_owned()),
        };
        let json = serde_json::to_string(&found).expect("serialises");
        let back = Discovered::parse_cached(&json).expect("round trips");
        assert_eq!(back.central_auth, found.central_auth);
        assert_eq!(back.orgs.len(), found.orgs.len());
    }
}
