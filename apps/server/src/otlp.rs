//! Authenticated OTLP ingest for out-of-cluster clients.
//!
//! The desktop, iOS and browser builds emit the same traces/logs/metrics
//! the server does, but they can't reach the cluster's collector — it is a
//! ClusterIP, deliberately. Publishing it would mean an endpoint anyone
//! can write into the trace store through, with its own credential to
//! provision and rotate.
//!
//! Instead the server proxies. Clients already hold a session token and
//! already talk to this host, so pointing
//! `OTEL_EXPORTER_OTLP_ENDPOINT` at `<server>/otlp` reuses the auth,
//! the TLS, and the CORS policy that are already there. The signal
//! paths converge in-cluster: a client span and the server span it
//! caused end up in the same Tempo.
//!
//! Mounted only when `TASK_OTLP_UPSTREAM` is set. A self-hoster with no
//! collector gets no route, not a route that 502s.

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};

use crate::AppState;
use crate::watch_bridge::bearer;

/// Where to forward, e.g. `http://otel-collector.observability.svc:4318`.
/// Absent → the routes are not mounted at all.
fn upstream() -> Option<String> {
    std::env::var("TASK_OTLP_UPSTREAM")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_owned())
        .filter(|v| !v.is_empty())
}

/// `/otlp/v1/{traces,logs,metrics}` — the three OTLP/HTTP signal paths,
/// exactly as the OTel SDKs construct them from a base endpoint. Returns
/// `None` when no upstream is configured.
pub fn otlp_router() -> Option<Router<AppState>> {
    upstream()?;
    Some(Router::new().route("/otlp/v1/{signal}", post(forward)))
}

/// Forward one OTLP payload upstream.
///
/// The body is passed through untouched — it is protobuf or JSON that the
/// collector parses, and this proxy has no business decoding it. Only the
/// content type follows it across, because that is what tells the
/// collector which of the two it is.
async fn forward(
    State(state): State<AppState>,
    Path(signal): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Only the three real signals — no arbitrary path pass-through into
    // an in-cluster service.
    if !matches!(signal.as_str(), "traces" | "logs" | "metrics") {
        return (StatusCode::NOT_FOUND, "unknown OTLP signal").into_response();
    }

    let Some(base) = upstream() else {
        return (StatusCode::NOT_FOUND, "OTLP ingest not configured").into_response();
    };

    if let Err(resp) = authenticate(&state, &headers).await {
        return resp;
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/x-protobuf")
        .to_owned();

    let client = reqwest::Client::new();
    match client
        .post(format!("{base}/v1/{signal}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(body)
        .send()
        .await
    {
        Ok(res) => {
            let status =
                StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let bytes = res.bytes().await.unwrap_or_default();
            (status, bytes).into_response()
        }
        Err(e) => {
            // Log, don't propagate detail: a client that can't export
            // telemetry should degrade quietly, and the failure is the
            // server's problem to see.
            tracing::warn!(%signal, error = %e, "OTLP forward failed");
            (StatusCode::BAD_GATEWAY, "collector unreachable").into_response()
        }
    }
}

/// A valid session token for ANY org this server hosts is enough.
///
/// Telemetry is not org-scoped data — a span says which build of the app
/// was slow, not what is in someone's vault — so the gate is "is this a
/// real signed-in client of this server", not "may this user read org
/// X". Keeping it that way avoids putting an org slug in the endpoint
/// URL, which would mean clients re-configuring their exporter on every
/// org switch.
async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let token = bearer(headers)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "missing bearer token").into_response())?;

    // Static ingest token for shipped clients. An OTLP exporter is built
    // at process start, before anyone has signed in, so it cannot carry a
    // session token — the clients bake this one in at build time
    // (`TASK_OTLP_TOKEN`, same mechanism as the Sentry DSN). It grants
    // exactly this: the ability to POST telemetry.
    let ingest_token = std::env::var("TASK_OTLP_TOKEN").unwrap_or_default();
    if !ingest_token.is_empty() && token == ingest_token {
        return Ok(());
    }

    for slug in state.org_slugs() {
        let Some(org) = state.org(&slug) else {
            continue;
        };
        if org
            .auth
            .auth
            .current_session(architect_auth::CurrentSession {
                token: token.clone(),
            })
            .await
            .is_ok()
        {
            return Ok(());
        }
    }
    Err((StatusCode::UNAUTHORIZED, "invalid session token").into_response())
}
