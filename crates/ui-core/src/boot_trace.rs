//! The boot of the app as ONE wide event, exported to the cluster.
//!
//! A page load is a chain — wasm compiled, discovery answered, session
//! restored, the org socket up, the shell painted — and the server sees
//! only the middle of it: one span per RPC, nothing about the seconds a
//! person spends looking at a blank tab before the first RPC goes out.
//! This module is the client's half. Phases are stamped as they happen
//! (`mark`); once the last one lands the whole boot is emitted once, as
//! one span with the phase durations as fields and one child span per
//! phase, through the server's authenticated OTLP proxy (`/otlp`), so it
//! sits in Tempo beside the server spans it caused.
//!
//! The span is the wide event: nothing here logs per phase. One
//! `tracing::info!` with every field fires alongside the export, so the
//! same numbers are in the console for whoever is watching it.
//!
//! Browser only. Native builds have `init_tracing_full` and export their
//! own spans; here every function is a no-op off wasm.

/// Stamp `name` at now, once. A second stamp of the same name is ignored:
/// the first time the shell paints is the one a boot is measured by.
pub fn mark(name: &'static str) {
    imp::mark(name);
}

/// Names the phases a boot is made of, in the order they happen. A boot
/// is complete — and exported — when every one of these has a mark.
pub const REQUIRED: &[&str] = &["wasm_main", "discovery", "session", "connection", "shell"];

#[cfg(target_arch = "wasm32")]
mod imp {
    use std::cell::RefCell;

    thread_local! {
        static MARKS: RefCell<Vec<(&'static str, f64)>> = const { RefCell::new(Vec::new()) };
        static SENT: RefCell<bool> = const { RefCell::new(false) };
    }

    fn now_ms() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now())
    }

    pub fn mark(name: &'static str) {
        let complete = MARKS.with(|m| {
            let mut m = m.borrow_mut();
            if m.iter().any(|(n, _)| *n == name) {
                return false;
            }
            m.push((name, now_ms()));
            super::REQUIRED
                .iter()
                .all(|r| m.iter().any(|(n, _)| n == r))
        });
        if complete && !SENT.with(|s| s.replace(true)) {
            let marks = MARKS.with(|m| m.borrow().clone());
            wasm_bindgen_futures::spawn_local(export(marks));
        }
    }

    /// 32 or 16 hex chars from the browser's RNG — a trace/span id.
    fn hex_id(bytes: usize) -> String {
        (0..bytes)
            .map(|_| format!("{:02x}", (js_sys::Math::random() * 256.0) as u8))
            .collect()
    }

    fn attr_str(key: &str, value: &str) -> serde_json::Value {
        serde_json::json!({ "key": key, "value": { "stringValue": value } })
    }

    fn attr_int(key: &str, value: f64) -> serde_json::Value {
        serde_json::json!({ "key": key, "value": { "intValue": (value.round() as i64).to_string() } })
    }

    async fn export(mut marks: Vec<(&'static str, f64)>) {
        marks.sort_by(|a, b| a.1.total_cmp(&b.1));
        let win = web_sys::window();
        let origin_ms = win
            .as_ref()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.time_origin());
        let route = win
            .as_ref()
            .and_then(|w| w.location().pathname().ok())
            .unwrap_or_default();
        let user_agent = win
            .as_ref()
            .map(|w| w.navigator().user_agent().unwrap_or_default())
            .unwrap_or_default();
        let total_ms = marks.last().map_or(0.0, |m| m.1);
        let at = |name: &str| marks.iter().find(|(n, _)| *n == name).map(|m| m.1);
        // Phase durations: each phase runs from the previous mark to its
        // own. `wasm_main` runs from navigation start (the browser's
        // zero), so it IS the fetch + compile + instantiate cost.
        let mut prev = 0.0;
        let mut phase_ms: Vec<(&'static str, f64, f64)> = Vec::new();
        for (name, t) in &marks {
            phase_ms.push((name, prev, *t));
            prev = *t;
        }
        let nanos = |ms: f64| format!("{}", ((origin_ms + ms) * 1_000_000.0) as u64);

        let trace_id = hex_id(16);
        let root_id = hex_id(8);
        let build = option_env!("TASK_BUILD_REV").unwrap_or("dev");

        let mut root_attrs = vec![
            attr_str("boot.route", &route),
            attr_str("boot.build", build),
            attr_int("boot.total_ms", total_ms),
        ];
        for (name, start, end) in &phase_ms {
            root_attrs.push(attr_int(&format!("boot.{name}_ms"), end - start));
        }
        let mut spans = vec![serde_json::json!({
            "traceId": trace_id,
            "spanId": root_id,
            "name": "web.boot",
            "kind": 1,
            "startTimeUnixNano": nanos(0.0),
            "endTimeUnixNano": nanos(total_ms),
            "attributes": root_attrs,
        })];
        for (name, start, end) in &phase_ms {
            spans.push(serde_json::json!({
                "traceId": trace_id,
                "spanId": hex_id(8),
                "parentSpanId": root_id,
                "name": format!("boot.{name}"),
                "kind": 1,
                "startTimeUnixNano": nanos(*start),
                "endTimeUnixNano": nanos(*end),
                "attributes": [attr_int("boot.phase_ms", end - start)],
            }));
        }
        let body = serde_json::json!({
            "resourceSpans": [{
                "resource": { "attributes": [
                    attr_str("service.name", "task-web"),
                    attr_str("service.version", env!("CARGO_PKG_VERSION")),
                    attr_str("deployment.environment", "prod"),
                    attr_str("browser.user_agent", &user_agent),
                ]},
                "scopeSpans": [{
                    "scope": { "name": "task-web/boot" },
                    "spans": spans,
                }],
            }],
        });

        // The console gets the same event once, as one line.
        tracing::info!(
            route = %route,
            build,
            total_ms = total_ms.round(),
            wasm_ms = at("wasm_main").unwrap_or_default().round(),
            discovery_ms = phase_ms.iter().find(|p| p.0 == "discovery").map_or(0.0, |p| (p.2 - p.1).round()),
            session_ms = phase_ms.iter().find(|p| p.0 == "session").map_or(0.0, |p| (p.2 - p.1).round()),
            connection_ms = phase_ms.iter().find(|p| p.0 == "connection").map_or(0.0, |p| (p.2 - p.1).round()),
            shell_ms = phase_ms.iter().find(|p| p.0 == "shell").map_or(0.0, |p| (p.2 - p.1).round()),
            "web.boot"
        );

        let Some(token) = crate::vox_session::bearer() else {
            return;
        };
        let base = crate::orgs::http_base();
        if base.is_empty() {
            return;
        }
        let Ok(headers) = web_sys::Headers::new() else {
            return;
        };
        let _ = headers.set("authorization", &format!("Bearer {token}"));
        let _ = headers.set("content-type", "application/json");
        let init = web_sys::RequestInit::new();
        init.set_method("POST");
        init.set_headers(&headers);
        init.set_body(&wasm_bindgen::JsValue::from_str(&body.to_string()));
        let Ok(req) =
            web_sys::Request::new_with_str_and_init(&format!("{base}/otlp/v1/traces"), &init)
        else {
            return;
        };
        if let Some(w) = win
            && let Err(e) = wasm_bindgen_futures::JsFuture::from(w.fetch_with_request(&req)).await
        {
            tracing::debug!(?e, "boot trace export failed");
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    pub fn mark(_name: &'static str) {}
}
