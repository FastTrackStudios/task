//! Thin shell — env-driven config, then hands off to `task_server::router`.

// The copy lints, denied here once the crate is clean of them
// (zero-copy audit S7, `docs/perf/zero-copy-audit.md`). They move into
// `[workspace.lints.clippy]` once every crate carries this.
#![deny(
    clippy::redundant_clone,
    clippy::implicit_clone,
    clippy::needless_pass_by_value,
    clippy::large_types_passed_by_value
)]

use std::net::SocketAddr;

use eyre::WrapErr;
use task_server::{AppState, router};
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // `--version`, `--help`, `service …`, `self-update …` — and a refusal
    // for anything unknown. First of all: a question about the binary
    // must never open its data or bind a port (see node_cli.rs).
    if task_server::node_cli::dispatch().await? {
        return Ok(());
    }
    // Operator subcommands run against this server's own data root and
    // exit; only a bare invocation boots the server. Handled before
    // telemetry so an admin command doesn't open exporters it will never
    // use.
    if task_server::admin_cli::dispatch().await? {
        return Ok(());
    }
    // `task-server peer`: this server as one more of a person's devices.
    if task_server::peer_cli::dispatch().await? {
        return Ok(());
    }

    // Sentry error/crash telemetry — hold the guard for all of `main`.
    let _sentry = architect_telemetry::init("task-server");

    // What is logged without RUST_LOG — and, below, what is exported.
    const LOG_DEFAULT: &str = "task_server=info,tower_http=info";
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| LOG_DEFAULT.into());
    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .with(architect_telemetry::tracing_layer());
    // OTLP traces/logs/metrics — inert unless OTEL_EXPORTER_OTLP_ENDPOINT
    // is set (the cluster points it at the collector; local runs stay
    // telemetry-free). The guard flushes the exporters on drop, so it
    // lives until the end of `main`.
    let _otel = match architect_telemetry::otel::init("task-server", LOG_DEFAULT) {
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

    // Every variable the server reads is a field of `ServerConfig`
    // (`task_server::config`), under its old name.
    let config = task_server::config::ServerConfig::load();
    let bind: SocketAddr = config
        .listen_bind()
        .parse()
        .wrap_err("invalid TASK_SERVER_BIND")?;

    // Per-org data root: `$TASK_DATA_ROOT/orgs/<slug>/` holds
    // this server's auth/timer/finance sqlites + vault. The
    // slug comes from `$TASK_SERVER_ORG`; if unset, falls back
    // to single-org auto-pick (or auto-bootstraps `default`).
    // PR 4 will scan the orgs dir and serve all of them at
    // `/org/<slug>/...`.
    let state = AppState::new(config.org.as_deref()).await?;

    // Every hosted org gets an iroh endpoint, and gets it *before* the
    // router is built: `iroh_host::start` installs the dialler on each
    // org's backend, and a router built first would dispatch into the
    // backends as they were. Held for the life of the process — dropping
    // the last endpoint handle stops the endpoint accepting.
    let iroh = task_server::iroh_host::start(&state).await;
    if let Some(host) = &iroh {
        for (slug, id) in host.ids() {
            info!(%slug, endpoint_id = %id, "reachable by endpoint id");
        }
        // And the other direction: sync is two pulls, so a server that
        // only serves the replica lane collects nothing its devices did
        // offline. This is the pull the server runs.
        task_server::device_sync::start(&state, host);
    }

    // Hold the construction scope so DB pools tear down in LIFO order
    // on shutdown; `router` takes ownership of `state`.
    let scope = state.scope.clone();
    // `router` serves everything a node serves — org-less `/media/…`
    // included (`task_server::media_router`), so a desktop node's
    // loopback answers the same URLs as this binary.
    let mut app = router(state);

    // Optional web bundle (the fasttrackstudio.app single-binary deploy):
    // the dx web build served from the very host that serves
    // `/org/{slug}/vox`, so the browser's same-origin vox URL works with no
    // second port. Off unless TASK_SERVER_WEB_DIR points somewhere.

    // TASK_SERVER_WEB_DIR — a dx web build (dir with index.html + wasm/ +
    // assets/). Asset dirs are served as files; everything else falls
    // through to index.html so the wasm router handles SPA deep links
    // like `/session/{org}/{collection}`. Vox/health/blob routes above
    // take precedence.
    if let Some(web_dir) = config.web_dir.clone() {
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
    drop(iroh);
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
    use architect_telemetry::otel::opentelemetry::{
        KeyValue, global,
        metrics::{Counter, Histogram},
    };
    use std::sync::OnceLock;

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
