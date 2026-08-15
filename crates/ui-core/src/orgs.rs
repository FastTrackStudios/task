//! Multi-org selection model.
//!
//! The server hosts several orgs (`/.well-known/task-server.json`
//! lists them). The UI lets you view **all** of them at once (the
//! default) or scope to a single org. The selection is held in a
//! `Signal<OrgSelection>` context; data fetchers resolve it to a list
//! of slugs via [`selected_slugs`] and fan out per org.
//!
//! Every feature UI crate needs this to scope its own fetches, so it
//! lives here rather than in the shell. **Discovery** (the well-known
//! fetch, which is `window.fetch` on wasm and `reqwest` on native)
//! stays in the shell's `orgs` module.

/// App-root context: the last org-discovery error (a native `fetch_orgs`
/// failure), surfaced in the Servers UI so a stuck "org discovery hasn't
/// resolved yet" shows *why* — a connect timeout, TLS error, 404, etc. —
/// instead of the app silently sitting on an empty org list. `None` = no
/// error / discovery succeeded.
#[derive(Clone, Copy)]
pub struct DiscoveryError(pub dioxus::prelude::Signal<Option<String>>);

/// One hosted org, as surfaced by the server's well-known endpoint.
// serde so the shell can keep the discovered org list in a boot cache:
// without it an offline client has no slug, and every org-scoped
// surface (email included) cannot even name what it wants.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct OrgMeta {
    pub slug: String,
    pub name: String,
    pub is_home: bool,
    /// Org's stable UUID (`org.toml` `id`). `None` for older servers
    /// that don't surface it. Needed by org-scoped services like the
    /// timer that key on `org_id`.
    pub id: Option<uuid::Uuid>,
    /// Plugin ids this org has turned off (`org.toml`
    /// `disabled_plugins`, relayed through the well-known doc). Empty
    /// for older servers — everything on. Resolve through
    /// [`plugin_set_for`] / [`active_plugin_set`] rather than reading
    /// raw (resolution handles unknown ids + always-on core).
    pub disabled_plugins: Vec<String>,
    /// Does the caller's session validate against THIS org?
    ///
    /// `None` when discovery ran without a token (signed out, or a
    /// server predating the field) — which must be treated as "show
    /// everything", because seeing an org is a precondition for signing
    /// into it. `Some(false)` is a positive answer: signed in, and not a
    /// member here.
    ///
    /// Each org has its own auth database server-side, so this is
    /// literally "my token validates there" rather than a membership
    /// table lookup. Drives [`OrgSelection::All`] meaning "all MY orgs"
    /// (issue #109 criterion 6).
    pub member: Option<bool>,
}

/// The orgs an `All` selection should actually span.
///
/// Membership is asked of each org **independently** — each org has its
/// own auth database, and the direction of travel is for an org to be
/// able to live on its own server entirely. So there is no global
/// membership table to consult and none is assumed: the question is
/// always "does my token validate *there*", answered per org (and, once
/// orgs are separate servers, per server, with the multi-server registry
/// already holding a token per entry).
///
/// The tags are homogeneous — discovery either sent a token (every entry
/// is `Some`) or did not (every entry is `None`) — which gives two clean
/// cases:
///
/// - **no token sent** (signed out, or a server predating the field):
///   every org, unchanged. Seeing an org is a precondition for signing
///   into it, so filtering here would make sign-in unreachable.
/// - **token sent**: exactly the orgs it validates against, *including
///   none of them*. A signed-in user who belongs to nothing here gets an
///   empty span rather than a fan-out that the permission gate will
///   refuse call by call — an empty state is a better answer than a wall
///   of errors, and it is the truthful one.
///
/// Note on where linked orgs enter this: NOT here. The app root folds
/// the identity locker's answer into each `OrgMeta::member` as the org
/// list resolves, because `org_list` is the Signal every consumer
/// reads. Consulting a global from inside this function instead looked
/// simpler and was wrong — Dioxus cannot see a static change, so the
/// switcher kept rendering a stale single org until something unrelated
/// forced a re-render. Keep this a pure function of its argument.
#[must_use]
pub fn my_orgs(orgs: &[OrgMeta]) -> Vec<OrgMeta> {
    my_orgs_with_links(orgs, &[])
}

/// [`my_orgs`], plus the orgs the identity locker holds a credential
/// for.
///
/// Discovery can only answer `member` for the ONE token it presented,
/// so on a server hosting several orgs it reports `false` for every org
/// that didn't issue that token — even ones this account is a full
/// member of. The locker is the other half of the answer: a link means
/// we hold a working credential for that org, which is the same claim
/// `member: true` makes.
///
/// Split out from [`my_orgs`] so the rule stays a pure function over
/// its inputs and can be tested without touching global session state.
#[must_use]
pub fn my_orgs_with_links(orgs: &[OrgMeta], linked: &[String]) -> Vec<OrgMeta> {
    let linked_here = |o: &OrgMeta| linked.iter().any(|s| s == &o.slug);
    if orgs.iter().all(|o| o.member.is_none()) {
        return orgs.to_vec();
    }
    orgs.iter()
        .filter(|o| o.member.unwrap_or(false) || linked_here(o))
        .cloned()
        .collect()
}

/// The effective plugin set of the org with `slug` (unknown slug or a
/// pre-plugin server = everything on; core is always on either way).
#[must_use]
pub fn plugin_set_for(orgs: &[OrgMeta], slug: &str) -> task_plugin::PluginSet {
    let disabled = orgs
        .iter()
        .find(|o| o.slug == slug)
        .map(|o| task_plugin::PluginChoice::Disabled(o.disabled_plugins.clone()));
    task_plugin::PluginSet::resolve(disabled.as_ref())
}

/// The plugin set the shell gates nav / widgets / routes on: the
/// ACTIVE org's (see [`active_slug`] — the selected org, or home under
/// "All"). Before discovery resolves this is "everything on", so
/// nothing flickers off while the org list loads.
#[must_use]
pub fn active_plugin_set(sel: &OrgSelection, orgs: &[OrgMeta]) -> task_plugin::PluginSet {
    plugin_set_for(orgs, &active_slug(sel, orgs))
}

/// What the org switcher is pointed at. `All` aggregates every hosted
/// org; `One` scopes to a single slug. Defaults to `All`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum OrgSelection {
    #[default]
    All,
    One(String),
}

impl OrgSelection {
    /// Display label for the switcher trigger.
    #[must_use]
    pub fn label(&self, orgs: &[OrgMeta]) -> String {
        match self {
            Self::All => "All organizations".to_string(),
            Self::One(slug) => orgs
                .iter()
                .find(|o| &o.slug == slug)
                .map_or_else(|| slug.clone(), |o| o.name.clone()),
        }
    }
}

/// Resolve a selection to the concrete slugs to fetch from. `All`
/// expands to every hosted org (home org first for stable ordering);
/// `One` is just that slug.
#[must_use]
pub fn selected_slugs(sel: &OrgSelection, orgs: &[OrgMeta]) -> Vec<String> {
    match sel {
        OrgSelection::One(slug) => vec![slug.clone()],
        OrgSelection::All => {
            // "All" means all MY orgs, not every org on the server
            // (#109 criterion 6). This is the fan-out choke point every
            // multi-org fetch goes through, so filtering here covers
            // `feeds::*` and the atom hooks in one place.
            let mine = my_orgs(orgs);
            let mut slugs: Vec<String> = mine.iter().map(|o| o.slug.clone()).collect();
            slugs.sort_by_key(|s| mine.iter().find(|o| &o.slug == s).map(|o| !o.is_home));
            slugs
        }
    }
}

/// Where a newly-created record should land: the selected org, or the
/// home org (falling back to the first hosted org) when viewing All.
#[must_use]
pub fn create_target(sel: &OrgSelection, orgs: &[OrgMeta]) -> String {
    match sel {
        OrgSelection::One(slug) => slug.clone(),
        OrgSelection::All => orgs
            .iter()
            .find(|o| o.is_home)
            .or_else(|| orgs.first())
            .map(|o| o.slug.clone())
            .unwrap_or_default(),
    }
}

/// The home org's slug (falls back to the first hosted org, then to
/// an empty string before discovery resolves).
#[must_use]
pub fn home_slug(orgs: &[OrgMeta]) -> String {
    orgs.iter()
        .find(|o| o.is_home)
        .or_else(|| orgs.first())
        .map(|o| o.slug.clone())
        .unwrap_or_default()
}

/// The single org that org-scoped surfaces (the vault page, the note
/// palette/omni-picker) should read from. When the switcher is scoped to
/// `One`, that org; otherwise the home org. `All` has no single vault to
/// show, so it falls back to home — pick a specific org in the switcher
/// to browse or search its vault. This is what lets the vault/palette
/// follow the org switcher instead of being pinned to the home org.
#[must_use]
pub fn active_slug(sel: &OrgSelection, orgs: &[OrgMeta]) -> String {
    match sel {
        OrgSelection::One(slug) => slug.clone(),
        OrgSelection::All => home_slug(orgs),
    }
}

/// HTTP(S) base derived from the configured vox WebSocket URL —
/// `ws://host/…` → `http://host`, `wss://` → `https://`.
#[must_use]
pub fn http_base() -> String {
    let v = crate::vox_session::vox_url();
    let v = v.trim_end_matches("/vox").trim_end_matches('/');
    if let Some(rest) = v.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = v.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod my_orgs_tests {
    use super::*;

    fn org(slug: &str, member: Option<bool>) -> OrgMeta {
        OrgMeta {
            slug: slug.to_owned(),
            name: slug.to_owned(),
            is_home: slug == "home",
            id: None,
            disabled_plugins: Vec::new(),
            member,
        }
    }

    fn slugs(v: &[OrgMeta]) -> Vec<&str> {
        v.iter().map(|o| o.slug.as_str()).collect()
    }

    #[test]
    fn signed_out_sees_every_org() {
        // Discovery ran without a token, so nothing is known. Filtering
        // here would hide the org you need to sign in to.
        let orgs = [org("home", None), org("other", None)];
        assert_eq!(slugs(&my_orgs(&orgs)), ["home", "other"]);
    }

    #[test]
    fn signed_in_sees_only_their_own() {
        // THE criterion-6 case: "All organizations" must not mean every
        // org on the server.
        let orgs = [
            org("home", Some(true)),
            org("someone-elses", Some(false)),
            org("also-mine", Some(true)),
        ];
        assert_eq!(slugs(&my_orgs(&orgs)), ["home", "also-mine"]);
    }

    #[test]
    fn a_member_of_nothing_gets_an_empty_span() {
        // Deliberately NOT a fallback to "show everything": an empty
        // state is truthful, and fanning out would just produce a wall of
        // refusals once the gate enforces.
        let orgs = [org("a", Some(false)), org("b", Some(false))];
        assert!(my_orgs(&orgs).is_empty());
    }

    #[test]
    fn a_linked_org_is_mine_even_when_discovery_says_otherwise() {
        // The production shape: one token, six orgs. Discovery can only
        // vouch for the org that issued it, so the other five come back
        // `member: false` despite the account being a full member —
        // holding a working credential for them is the same claim.
        let orgs = [
            org("codywright", Some(true)),
            org("fasttrackstudios", Some(false)),
            org("someone-elses", Some(false)),
        ];
        let linked = ["fasttrackstudios".to_owned()];
        assert_eq!(
            slugs(&my_orgs_with_links(&orgs, &linked)),
            ["codywright", "fasttrackstudios"],
            "a link is a credential; an org we hold none for stays hidden"
        );
    }

    #[test]
    fn links_do_not_resurrect_orgs_when_signed_out() {
        // No token sent → every entry `None` → show everything, and the
        // link list must not change that shape.
        let orgs = [org("a", None), org("b", None)];
        assert_eq!(
            slugs(&my_orgs_with_links(&orgs, &["a".to_owned()])),
            ["a", "b"]
        );
    }

    #[test]
    fn all_selection_spans_only_my_orgs() {
        // `selected_slugs` is the choke point every multi-org fetch uses,
        // so this is what actually stops the anonymous-style fan-out.
        let orgs = [
            org("someone-elses", Some(false)),
            org("home", Some(true)),
        ];
        let spanned = selected_slugs(&OrgSelection::All, &orgs);
        assert_eq!(spanned, vec!["home".to_string()]);
    }

    #[test]
    fn explicitly_selecting_one_org_is_untouched() {
        // An explicit pick stays an explicit pick — the server decides
        // whether the data comes back, not the switcher.
        let orgs = [org("someone-elses", Some(false))];
        assert_eq!(
            selected_slugs(&OrgSelection::One("someone-elses".into()), &orgs),
            vec!["someone-elses".to_string()]
        );
    }

    #[test]
    fn home_still_sorts_first_within_my_orgs() {
        let orgs = [
            org("zzz", Some(true)),
            org("home", Some(true)),
            org("nope", Some(false)),
        ];
        assert_eq!(selected_slugs(&OrgSelection::All, &orgs), ["home", "zzz"]);
    }
}
