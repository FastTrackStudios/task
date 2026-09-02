//! Who counts as an **operator** — someone allowed at surfaces that span
//! every org on this server (cluster telemetry, CPU profiles, thread
//! tables). One gate, shared by the MCP `telemetry_*` tools
//! ([`crate::mcp`]) and the `/server/debug/*` HTTP routes
//! ([`crate::debug_profile`]), so the two lanes can never drift apart on
//! who they let in.

use axum::http::HeaderMap;

use crate::AppState;

/// Is this caller an operator — allowed to read what spans every org?
/// The static `TASK_MCP_TOKEN` is one by definition; a session is one
/// when its principal holds `admin` (or `owner`) in the HOME org, read
/// from the memberships table when it exists and from the home org's
/// own role column otherwise (the two places `admin set-role` /
/// `adopt-principal` write).
///
/// Sets `auth.principal_kind` on the span for the static token; the
/// session paths leave it to the resolver that named the user.
pub(crate) async fn is_operator(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(token) = crate::watch_bridge::bearer(headers) else {
        return false;
    };
    let static_token = std::env::var("TASK_MCP_TOKEN").unwrap_or_default();
    if !static_token.is_empty() && token == static_token {
        architect_telemetry::wide::set("auth.principal_kind", "static_token");
        return true;
    }
    let Some(home_slug) = state.home_slug() else {
        return false;
    };
    let Some(home) = state.org(&home_slug) else {
        return false;
    };
    // An operator role on the home org's own account, when the token
    // is one of its sessions…
    if let Ok(bundle) = home
        .auth
        .auth
        .current_session(architect_auth::CurrentSession {
            token: token.clone(),
        })
        .await
        && operator_role(bundle.user.role.as_deref())
    {
        architect_telemetry::wide::set("auth.principal_kind", "user");
        return true;
    }
    // …or on the home org's membership row, for any principal the home
    // org recognises — its own accounts and, with central auth, the
    // issuer's (`central_auth::home_principal`). An `owner` outranks an
    // `admin`; refusing one would refuse the person who runs the server.
    if let Some(identity) = &state.home_identity
        && let Some(user_id) = crate::central_auth::home_principal(state, &token).await
        && let Ok(Some(m)) = identity.memberships.role_for(user_id, &home_slug).await
        && operator_role(m.role.as_deref())
    {
        architect_telemetry::wide::set("auth.principal_kind", "user");
        return true;
    }
    false
}

fn operator_role(role: Option<&str>) -> bool {
    matches!(role, Some("admin" | "owner"))
}
