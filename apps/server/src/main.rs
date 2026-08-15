//! Thin shell — env-driven config, then hands off to `task_server::router`.

use std::net::SocketAddr;

use eyre::WrapErr;
use task_server::{AppState, router};
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Operator subcommands run against this server's own data root and
    // exit; only a bare invocation boots the server. Handled before
    // telemetry so an admin command doesn't open exporters it will never
    // use.
    if task_server::admin_cli::dispatch().await? {
        return Ok(());
    }

    // Sentry error/crash telemetry — hold the guard for all of `main`.
    let _sentry = architect_telemetry::init("task-server");

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "task_server=info,tower_http=info".into());
    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .with(architect_telemetry::tracing_layer());
    // OTLP traces/logs/metrics — inert unless OTEL_EXPORTER_OTLP_ENDPOINT
    // is set (the cluster points it at the collector; local runs stay
    // telemetry-free). The guard flushes the exporters on drop, so it
    // lives until the end of `main`.
    let _otel = match architect_telemetry::otel::init("task-server") {
        Some((guard, layers)) => {
            registry.with(layers).init();
            info!("OTLP export enabled");
            Some(guard)
        }
        None => {
            registry.init();
            None
        }
    };

    let bind: SocketAddr = std::env::var("TASK_SERVER_BIND")
        .unwrap_or_else(|_| "0.0.0.0:9090".into())
        .parse()
        .wrap_err("invalid TASK_SERVER_BIND")?;

    // Per-org data root: `$TASK_DATA_ROOT/orgs/<slug>/` holds
    // this server's auth/timer/finance sqlites + vault. The
    // slug comes from `$TASK_SERVER_ORG`; if unset, falls back
    // to single-org auto-pick (or auto-bootstraps `default`).
    // PR 4 will scan the orgs dir and serve all of them at
    // `/org/<slug>/...`.
    let org_slug = std::env::var("TASK_SERVER_ORG").ok();
    let state = AppState::new(org_slug.as_deref()).await?;
    // Hold the construction scope so DB pools tear down in LIFO order
    // on shutdown; `router` takes ownership of `state`.
    let scope = state.scope.clone();
    // Base dir for the always-on media route below (`<data_root>/orgs`).
    let orgs_dir = state.data_root.orgs_dir();
    let mut app = router(state);

    // Song media at `/media/…`, ALWAYS ON, filesystem-first: a request for
    // `/media/songs/<slug>/manifest.json` (or a stem) is resolved against
    // EVERY org's `resources/` dir — the first org that has the file wins.
    // No ingest, no content-addressing, no env var: drop a file into an
    // org's `resources/` (e.g. over network storage) and it's served. Same
    // `/media/…` URLs the whole UI already builds (org-less, same-origin).
    {
        use axum::extract::Path as AxPath;
        use axum::http::{StatusCode, header};
        use axum::response::IntoResponse;
        use axum::routing::get;
        let media_orgs = orgs_dir.clone();
        app = app.route(
            "/media/{*path}",
            get(move |AxPath(rel): AxPath<String>| {
                let orgs = media_orgs.clone();
                async move {
                    // Reject traversal; `rel` is already percent-decoded.
                    if rel.split('/').any(|seg| seg == ".." || seg.is_empty()) {
                        return StatusCode::NOT_FOUND.into_response();
                    }
                    let mut hit = None;
                    if let Ok(entries) = std::fs::read_dir(&orgs) {
                        for e in entries.flatten() {
                            let cand = e.path().join("resources").join(&rel);
                            if cand.is_file() {
                                hit = Some(cand);
                                break;
                            }
                        }
                    }
                    let Some(path) = hit else {
                        return StatusCode::NOT_FOUND.into_response();
                    };
                    match tokio::fs::read(&path).await {
                        Ok(bytes) => {
                            let ct = match path.extension().and_then(|x| x.to_str()) {
                                Some("json") => "application/json",
                                Some("ogg") => "audio/ogg",
                                Some("wav") => "audio/wav",
                                Some("mp3") => "audio/mpeg",
                                Some("webm") => "audio/webm",
                                Some("kf") | Some("txt") | Some("md") => {
                                    "text/plain; charset=utf-8"
                                }
                                _ => "application/octet-stream",
                            };
                            ([(header::CONTENT_TYPE, ct)], bytes).into_response()
                        }
                        Err(_) => StatusCode::NOT_FOUND.into_response(),
                    }
                }
            }),
        );
        info!(orgs_dir = %orgs_dir.display(), "serving song media at /media (org-resolved)");
    }

    // Optional web bundle (the fasttrackstudio.app single-binary deploy):
    // the dx web build served from the very host that serves
    // `/org/{slug}/vox`, so the browser's same-origin vox URL works with no
    // second port. Off unless TASK_SERVER_WEB_DIR points somewhere.

    // TASK_SERVER_WEB_DIR — a dx web build (dir with index.html + wasm/ +
    // assets/). Asset dirs are served as files; everything else falls
    // through to index.html so the wasm router handles SPA deep links
    // like `/session/{org}/{collection}`. Vox/health/blob routes above
    // take precedence.
    if let Ok(web_dir) = std::env::var("TASK_SERVER_WEB_DIR") {
        use axum::response::Html;
        use tower_http::services::ServeDir;
        let web = std::path::PathBuf::from(&web_dir);
        let index_body = std::fs::read_to_string(web.join("index.html")).unwrap_or_default();
        app = app
            .nest_service("/wasm", ServeDir::new(web.join("wasm")))
            .nest_service("/assets", ServeDir::new(web.join("assets")))
            .fallback(move || {
                let b = index_body.clone();
                async move { Html(b) }
            });
        info!(%web_dir, "serving web bundle (SPA) same-origin");
    }

    // HTTP observability, outermost so it covers every route added above
    // (including `/media` and the SPA fallback): one span per request
    // (exported as an OTel trace when OTLP is on) plus request-count and
    // duration metrics. Labels carry method + MATCHED path — never the
    // raw URI, which holds org slugs and note paths, and never an
    // unbounded label set.
    let app = app.layer(axum::middleware::from_fn(http_metrics)).layer(
        tower_http::trace::TraceLayer::new_for_http()
            .make_span_with(|req: &axum::http::Request<_>| {
                let route = req
                    .extensions()
                    .get::<axum::extract::MatchedPath>()
                    .map(|p| p.as_str().to_owned())
                    .unwrap_or_else(|| "<unmatched>".to_owned());
                tracing::info_span!(
                    "http.request",
                    method = %req.method(),
                    route,
                    status = tracing::field::Empty,
                )
            })
            .on_response(
                |res: &axum::http::Response<_>,
                 _latency: std::time::Duration,
                 span: &tracing::Span| {
                    span.record("status", res.status().as_u16());
                },
            ),
    );

    info!(%bind, "listening");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("shutdown — closing backend resources");
    scope.close().await;
    Ok(())
}

/// Per-request RED metrics: `http.server.request.duration` (histogram,
/// seconds) and `http.server.requests` (counter), labelled by method,
/// matched route, and status code. Instruments are created once and
/// reused; when OTLP is off the global meter is a no-op, so this costs
/// a couple of atomic bumps.
async fn http_metrics(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use std::sync::OnceLock;
    use architect_telemetry::otel::opentelemetry::{
        KeyValue, global,
        metrics::{Counter, Histogram},
    };

    static INSTRUMENTS: OnceLock<(Histogram<f64>, Counter<u64>)> = OnceLock::new();
    let (duration, count) = INSTRUMENTS.get_or_init(|| {
        let meter = global::meter("task-server");
        (
            meter
                .f64_histogram("http.server.request.duration")
                .with_unit("s")
                .with_description("HTTP server request duration")
                .build(),
            meter
                .u64_counter("http.server.requests")
                .with_description("HTTP server requests handled")
                .build(),
        )
    });

    let method = req.method().to_string();
    let route = req
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "<unmatched>".to_owned());

    let started = std::time::Instant::now();
    let res = next.run(req).await;
    let elapsed = started.elapsed().as_secs_f64();

    let labels = [
        KeyValue::new("http.request.method", method),
        KeyValue::new("http.route", route),
        KeyValue::new(
            "http.response.status_code",
            i64::from(res.status().as_u16()),
        ),
    ];
    duration.record(elapsed, &labels);
    count.add(1, &labels);
    res
}

/// Resolve when the process receives Ctrl-C (or SIGTERM on unix), so
/// axum stops accepting and in-flight requests drain before the scope
/// finalizers run.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
