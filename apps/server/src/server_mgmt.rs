//! `OrgManagementService` server-side impl.
//!
//! Mounted at `/server/vox` (one endpoint per task-server
//! process, not per-org). Handles `create_org` calls: writes a
//! fresh `<data_root>/orgs/<slug>/` dir on disk, opens its
//! per-org SQLite DBs + runs their migrations, then hot-adds
//! the resulting [`crate::OrgAppState`] to [`AppState::orgs`]
//! so the next `/org/<slug>/...` request routes to it without
//! a server restart.
//!
//! ## Authorization
//!
//! - **Bootstrap mode** (no orgs hosted yet): unauthenticated
//!   `create_org` is allowed. The first claimed slot is
//!   typically the user's home org.
//! - **Normal mode**: `session_token` must validate against
//!   the home org's `auth.sqlite` via
//!   `ArchitectAuth::current_session`.

use std::sync::Arc;

use org_proto::{
    CreateOrgRequest, OrgManagementError, OrgManagementService, OrgManifest, OrgRoot,
    PersonalOrgRequest,
};

use crate::{AppState, AuthState, build_org_state};

/// Backend that knows how to scaffold + register orgs against
/// a live [`AppState`]. Holds an `Arc<AppState>` so mutations
/// to the orgs map land on the same lock the request handlers
/// read from.
#[derive(Clone, architect::HasDispatcher)]
pub struct OrgManagementImpl {
    state: Arc<AppState>,
    /// True when served over the in-process `LocalServer` (embedded
    /// CLI): the caller already owns the data root on disk, so session
    /// validation is skipped — same trust model as the per-org
    /// embedded transport, which mounts the org router with no auth
    /// gate at all.
    local_trusted: bool,
}

impl OrgManagementImpl {
    #[must_use]
    pub fn new(state: AppState) -> Self {
        Self {
            state: Arc::new(state),
            local_trusted: false,
        }
    }

    /// In-process transport constructor — skips session validation
    /// (see `local_trusted`).
    #[must_use]
    pub fn new_local_trusted(state: AppState) -> Self {
        Self {
            state: Arc::new(state),
            local_trusted: true,
        }
    }
}

impl OrgManagementService for OrgManagementImpl {
    fn create_org(&self, req: CreateOrgRequest) -> Result<OrgManifest, OrgManagementError> {
        // Authorization. Bootstrap path is permissive; normal
        // mode requires a valid session token against the
        // home org's auth.sqlite.
        if !self.local_trusted && !self.state.is_bootstrap() {
            let home_slug = self.state.home_slug().ok_or_else(|| {
                OrgManagementError::Unauthorized(
                    "server has orgs but no home org — cannot validate".into(),
                )
            })?;
            if req.session_token.is_empty() {
                return Err(OrgManagementError::Unauthorized(
                    "missing session token (bootstrap mode is over — sign in to the home org)"
                        .into(),
                ));
            }
            let home = self.state.org(&home_slug).ok_or_else(|| {
                OrgManagementError::Unauthorized(format!(
                    "home org `{home_slug}` not in live dispatcher"
                ))
            })?;
            let _ = home;
            let state = self.state.clone();
            let token = req.session_token.clone();
            tokio::runtime::Handle::current()
                .block_on(async move { crate::central_auth::home_principal(&state, &token).await })
                .ok_or_else(|| OrgManagementError::Unauthorized("invalid session token".into()))?;
        }

        self.scaffold(&req.slug, &req.display_name, req.is_home)
    }

    fn list_orgs(&self) -> Result<Vec<OrgManifest>, OrgManagementError> {
        let slugs = self.state.org_slugs();
        let mut out = Vec::with_capacity(slugs.len());
        for slug in slugs {
            let manifest =
                self.state.data_root.org(&slug).manifest().map_err(|e| {
                    OrgManagementError::Io(format!("load manifest for `{slug}`: {e}"))
                })?;
            out.push(manifest);
        }
        Ok(out)
    }

    fn ensure_personal_org(
        &self,
        req: PersonalOrgRequest,
    ) -> Result<OrgManifest, OrgManagementError> {
        self.provision_personal_org(req)
    }
}

impl OrgManagementImpl {
    /// Create the org on disk, open its stores and hot-add it to the
    /// live dispatcher. Authorization is the caller's business: this is
    /// reached both from `create_org`, which fences on the home org,
    /// and from `ensure_personal_org`, which fences on the slug being
    /// derived from the asker rather than chosen by them.
    fn scaffold(
        &self,
        req_slug: &str,
        display_name: &str,
        is_home: bool,
    ) -> Result<OrgManifest, OrgManagementError> {
        // Enforce single-home invariant up front so we don't
        // half-create + leave a partial org on disk.
        if is_home && self.state.home_slug().is_some() {
            return Err(OrgManagementError::HomeExists(
                self.state.home_slug().unwrap_or_default(),
            ));
        }

        // Scaffold the dir + write the manifest. `init_org`
        // validates the slug and refuses to overwrite an
        // existing org dir.
        let org_root: OrgRoot = self
            .state
            .data_root
            .init_org(req_slug, display_name, is_home)
            .map_err(|e| match e {
                org_proto::RootError::InvalidSlug { reason, .. } => {
                    OrgManagementError::InvalidSlug(reason.to_string())
                }
                org_proto::RootError::AlreadyExists { slug, .. } => {
                    OrgManagementError::AlreadyExists(slug)
                }
                other => OrgManagementError::Io(other.to_string()),
            })?;

        // Open + migrate the org's auth.sqlite. The keypair
        // mirrors the parent AppState — blob signing stays
        // consistent across orgs.
        let auth_db_url = format!("sqlite://{}?mode=rwc", org_root.auth_db().display());
        let keypair = self.state.keypair.clone();
        let scope = self.state.scope.clone();
        // The deployment's storage coordinator — a new org gets a view of
        // the same registry every other org uses, never its own.
        let storage = self.state.storage.clone();
        let slug = org_root.slug().to_owned();
        // An org created at runtime joins the same cross-org identity as
        // the ones scanned at boot — otherwise it would be the one org a
        // home principal could never reach without a restart.
        let home_identity = self.state.home_identity.clone();
        let built = tokio::runtime::Handle::current().block_on(async move {
            let auth = AuthState::open(&auth_db_url, &crate::auth_secret())
                .await
                .map_err(|e| OrgManagementError::Internal(format!("open auth: {e}")))?;
            build_org_state(
                auth,
                &keypair,
                org_root,
                &scope,
                &storage,
                home_identity.as_ref(),
            )
            .await
            .map_err(|e| OrgManagementError::Internal(format!("build org: {e}")))
        })?;

        self.state
            .insert_org(slug.clone(), built)
            .map_err(|e| OrgManagementError::Internal(e.into()))?;

        let manifest = self
            .state
            .data_root
            .org(&slug)
            .manifest()
            .map_err(|e| OrgManagementError::Io(format!("reload manifest: {e}")))?;
        Ok(manifest)
    }

    /// The body of [`OrgManagementService::ensure_personal_org`].
    fn provision_personal_org(
        &self,
        req: PersonalOrgRequest,
    ) -> Result<OrgManifest, OrgManagementError> {
        // No bootstrap path: a personal org is never the home org, and
        // a server with no home org has no memberships table to write
        // the owner row into. Refuse rather than quietly claim the
        // home slot for whoever asked first.
        let home_slug = self.state.home_slug().ok_or_else(|| {
            OrgManagementError::Unauthorized(
                "server has no home org yet — nothing to anchor an account to".into(),
            )
        })?;
        let home = self.state.home_identity.clone().ok_or_else(|| {
            OrgManagementError::Internal(format!("home org `{home_slug}` has no identity store"))
        })?;

        let state = self.state.clone();
        let token = req.session_token.clone();
        let (user_id, email) = tokio::runtime::Handle::current()
            .block_on(async move { crate::central_auth::identify_unfenced(&state, &token).await })
            .ok_or_else(|| OrgManagementError::Unauthorized("invalid session token".into()))?;

        let slug = personal_slug(user_id, email.as_deref());

        // Idempotence, and the collision fence in one check. If the org
        // is already on disk it is either this principal's (there is a
        // row, so hand it back) or someone else's (there is not, and a
        // derived slug must never adopt an existing org).
        if self.state.org(&slug).is_some() {
            let memberships = home.memberships.clone();
            let existing = tokio::runtime::Handle::current()
                .block_on({
                    let slug = slug.clone();
                    async move { memberships.role_for(user_id, &slug).await }
                })
                .map_err(|e| OrgManagementError::Internal(format!("membership lookup: {e}")))?;
            if existing.is_none() {
                return Err(OrgManagementError::AlreadyExists(slug));
            }
            return self
                .state
                .data_root
                .org(&slug)
                .manifest()
                .map_err(|e| OrgManagementError::Io(format!("load manifest: {e}")));
        }

        // Same scaffold the CLI gets, minus the choice of slug and with
        // `is_home` forced off — `create_org` is the only way to claim
        // the home slot, and it stays fenced.
        let display_name = personal_display_name(email.as_deref());
        let manifest = self.scaffold(&slug, &display_name, false)?;

        // The org exists; without this row it is an org the person who
        // asked for it cannot read. Written after creation so a failed
        // scaffold leaves no membership pointing at nothing.
        let memberships = home.memberships.clone();
        tokio::runtime::Handle::current()
            .block_on({
                let slug = slug.clone();
                async move { memberships.upsert(user_id, &slug, Some("owner")).await }
            })
            .map_err(|e| OrgManagementError::Internal(format!("grant ownership: {e}")))?;

        Ok(manifest)
    }
}

/// The slug of a principal's personal org.
///
/// A function of the principal, never of the request — that is the
/// property `ensure_personal_org` rests on. Two calls with the same
/// account produce the same slug, so provisioning is idempotent without
/// storing a mapping, and no caller can steer it at somebody else's org.
///
/// The email supplies the readable half, because the slug shows up in
/// every `/org/<slug>/` URL the person will ever see. The id supplies
/// the unique half: two accounts can hold the same local part on
/// different domains, and one of them must not get the other's org. An
/// account with no email keeps the id alone, which is ugly and correct.
fn personal_slug(user_id: uuid::Uuid, email: Option<&str>) -> String {
    let suffix = &user_id.simple().to_string()[..8];
    let stem = email
        .and_then(|e| e.split('@').next())
        .map(sanitize_slug_stem)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "person".to_owned());
    format!("{stem}-{suffix}")
}

/// Reduce a local part to the org-slug alphabet: lowercase `[a-z0-9-]`,
/// no runs of `-`, no leading or trailing `-`, capped well short of the
/// 64-char limit so the id suffix always fits.
fn sanitize_slug_stem(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 24 {
            break;
        }
    }
    out.trim_matches('-').to_owned()
}

/// What the org calls itself in the switcher.
fn personal_display_name(email: Option<&str>) -> String {
    match email
        .and_then(|e| e.split('@').next())
        .filter(|s| !s.is_empty())
    {
        Some(local) => format!("{local}'s workspace"),
        None => "Personal workspace".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{personal_display_name, personal_slug, sanitize_slug_stem};
    use uuid::Uuid;

    fn id(s: &str) -> Uuid {
        Uuid::parse_str(s).expect("valid uuid")
    }

    /// The property the whole design rests on: same account, same slug,
    /// every time, with no stored mapping to drift.
    #[test]
    fn the_slug_is_a_function_of_the_account() {
        let user = id("3f2a9155-0000-4000-8000-000000000001");
        let first = personal_slug(user, Some("acodywright@gmail.com"));
        let again = personal_slug(user, Some("acodywright@gmail.com"));
        assert_eq!(first, again);
        assert_eq!(first, "acodywright-3f2a9155");
    }

    /// Two people can hold the same local part on different providers.
    /// If the slug came from the email alone, the second would be handed
    /// the first one's org.
    #[test]
    fn the_same_local_part_on_two_domains_gets_two_orgs() {
        let one = personal_slug(
            id("11111111-0000-4000-8000-000000000001"),
            Some("cody@a.com"),
        );
        let two = personal_slug(
            id("22222222-0000-4000-8000-000000000002"),
            Some("cody@b.com"),
        );
        assert_ne!(one, two);
    }

    /// Every derived slug has to satisfy the same rule `init_org`
    /// enforces, or provisioning fails at the filesystem instead of
    /// here: `[a-z0-9-]`, 1-64, no leading or trailing dash.
    #[test]
    fn hostile_local_parts_still_produce_a_legal_slug() {
        let user = id("abcdef01-0000-4000-8000-000000000003");
        for raw in [
            "..dots..",
            "UPPER",
            "with spaces",
            "a+plus@tagged",
            "-leading",
            "trailing-",
            "sym!@#$%^&*()bols",
            "ünïcödé",
            "",
            &"x".repeat(200),
        ] {
            let slug = personal_slug(user, Some(&format!("{raw}@example.com")));
            assert!(!slug.is_empty(), "{raw}: empty");
            assert!(slug.len() <= 64, "{raw}: too long ({})", slug.len());
            assert!(!slug.starts_with('-'), "{raw}: leading dash in {slug}");
            assert!(!slug.ends_with('-'), "{raw}: trailing dash in {slug}");
            assert!(
                slug.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{raw}: illegal character in {slug}"
            );
        }
    }

    /// An account the issuer gave us no email for still gets an org.
    #[test]
    fn an_account_with_no_email_is_still_provisionable() {
        let slug = personal_slug(id("99999999-0000-4000-8000-000000000004"), None);
        assert_eq!(slug, "person-99999999");
        assert_eq!(personal_display_name(None), "Personal workspace");
    }

    #[test]
    fn a_stem_of_only_punctuation_is_empty_rather_than_a_dash() {
        assert_eq!(sanitize_slug_stem("..."), "");
        assert_eq!(sanitize_slug_stem("-"), "");
    }

    #[test]
    fn the_display_name_reads_as_a_person() {
        assert_eq!(
            personal_display_name(Some("acodywright@gmail.com")),
            "acodywright's workspace"
        );
    }
}
