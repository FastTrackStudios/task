//! Read-side client for the cluster's telemetry backends — Tempo
//! (traces) and Loki (logs) — shaped for an LLM reader.
//!
//! The server *emits* telemetry through `architect-telemetry` whenever
//! `OTEL_EXPORTER_OTLP_ENDPOINT` is set; this module is the other
//! direction. It exists so an agent on the account-scoped MCP lane
//! (`telemetry_*` tools in [`crate::mcp`]) can answer "why did that
//! request fail" from the same spans and log lines an operator would
//! read in Grafana, without a `kubectl` hop.
//!
//! # Off unless configured
//!
//! `TASK_TELEMETRY_TEMPO_URL` and `TASK_TELEMETRY_LOKI_URL` are both
//! optional. An unset backend answers [`QueryError::Unconfigured`], and
//! the MCP lane hides the tools entirely when neither is set — a
//! self-hoster without a stack never sees them.
//!
//! # Shaping
//!
//! Raw Tempo/Loki JSON is verbose (OTLP attribute arrays, nanosecond
//! strings, nested batches). Everything here flattens to rows an agent
//! can scan: attributes become `key: value` strings, timestamps become
//! RFC 3339, ANSI escapes are stripped from log lines, and the total
//! output is capped (`truncated: true` when rows were dropped) so one
//! noisy query cannot swallow the agent's context window.

use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Env var naming the Tempo HTTP API base (e.g. `http://tempo.observability.svc:3200`).
pub const TEMPO_URL_VAR: &str = "TASK_TELEMETRY_TEMPO_URL";
/// Env var naming the Loki HTTP API base (e.g. `http://loki.observability.svc:3100`).
pub const LOKI_URL_VAR: &str = "TASK_TELEMETRY_LOKI_URL";

/// Default lookback when a tool call carries no `since`.
pub const DEFAULT_SINCE: &str = "1h";
/// Default and maximum row counts a query may ask a backend for.
pub const DEFAULT_LIMIT: usize = 20;
pub const MAX_LIMIT: usize = 200;
/// Upper bound on the serialized size of a shaped result, in bytes.
/// Roughly 12k tokens — enough to read a trace, small enough that a
/// runaway `{}` query does not end the conversation.
pub const OUTPUT_CAP_BYTES: usize = 48 * 1024;

/// Which backends this process knows about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TelemetryConfig {
    pub tempo: Option<String>,
    pub loki: Option<String>,
}

impl TelemetryConfig {
    /// Read both URLs from the environment. Blank values count as unset;
    /// a trailing slash is dropped so path joins stay predictable.
    #[must_use]
    pub fn from_env() -> Self {
        let read = |var: &str| {
            std::env::var(var)
                .ok()
                .map(|s| s.trim().trim_end_matches('/').to_owned())
                .filter(|s| !s.is_empty())
        };
        Self {
            tempo: read(TEMPO_URL_VAR),
            loki: read(LOKI_URL_VAR),
        }
    }

    /// True when at least one backend is configured — the condition
    /// under which the MCP lane advertises the `telemetry_*` tools.
    #[must_use]
    pub fn any(&self) -> bool {
        self.tempo.is_some() || self.loki.is_some()
    }

    /// The configuration as an agent may see it: URLs with any
    /// `user:pass@` stripped, so a basic-auth'd backend never leaks its
    /// credential through `telemetry_status`.
    #[must_use]
    pub fn describe(&self) -> Value {
        json!({
            "tempo": self.tempo.as_deref().map(redact_userinfo),
            "loki": self.loki.as_deref().map(redact_userinfo),
        })
    }
}

/// Drop `user:pass@` from a URL's authority, keeping everything else.
#[must_use]
pub fn redact_userinfo(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    format!("{scheme}://{host}{path}")
}

/// Why a query produced no rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// The backend this query needs has no URL. Carries the env var
    /// that would configure it.
    Unconfigured(&'static str),
    /// The caller's arguments were unusable (bad `since`, empty query).
    BadRequest(String),
    /// The backend answered with an error or did not answer at all.
    Upstream(String),
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unconfigured(var) => write!(
                f,
                "telemetry backend not configured on this server — set `{var}` to enable it"
            ),
            Self::BadRequest(m) => write!(f, "{m}"),
            Self::Upstream(m) => write!(f, "telemetry backend error: {m}"),
        }
    }
}

impl QueryError {
    /// The `telemetry.outcome` value this error records on the span.
    #[must_use]
    pub const fn outcome(&self) -> &'static str {
        match self {
            Self::Unconfigured(_) => "unconfigured",
            Self::BadRequest(_) => "bad_request",
            Self::Upstream(_) => "upstream_error",
        }
    }
}

// ── `since` ───────────────────────────────────────────────────────

/// Parse a lookback like `15m`, `2h`, `1d`, `90s` into a duration.
///
/// Deliberately small: the agent types these, and a lookback is the
/// only time argument the tools take. Absolute ranges can wait for a
/// caller that needs them.
pub fn parse_since(s: &str) -> Result<Duration, QueryError> {
    let s = s.trim();
    let (digits, unit) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => s.split_at(i),
        None => (s, ""),
    };
    let n: u64 = digits.parse().map_err(|_| {
        QueryError::BadRequest(format!(
            "bad `since` `{s}`: expected like `15m`, `2h`, `1d`"
        ))
    })?;
    if n == 0 {
        return Err(QueryError::BadRequest(format!(
            "bad `since` `{s}`: must be > 0"
        )));
    }
    let secs = match unit.trim() {
        "s" => n,
        "m" | "" => n * 60,
        "h" => n * 3_600,
        "d" => n * 86_400,
        "w" => n * 7 * 86_400,
        other => {
            return Err(QueryError::BadRequest(format!(
                "bad `since` unit `{other}` in `{s}`: use s, m, h, d or w"
            )));
        }
    };
    // 30 days: past that the query is a data export, not a debugging
    // question, and Tempo's block search gets slow.
    if secs > 30 * 86_400 {
        return Err(QueryError::BadRequest(format!(
            "`since` `{s}` exceeds the 30d maximum"
        )));
    }
    Ok(Duration::from_secs(secs))
}

/// `(start, end)` in unix seconds for a lookback ending now.
fn window_secs(since: Duration) -> (u64, u64) {
    let end = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    (end.saturating_sub(since.as_secs()), end)
}

/// Clamp a caller-supplied limit into `1..=MAX_LIMIT`, defaulting.
#[must_use]
pub fn clamp_limit(limit: Option<u64>) -> usize {
    match limit {
        None => DEFAULT_LIMIT,
        Some(0) => 1,
        Some(n) => usize::try_from(n).unwrap_or(MAX_LIMIT).min(MAX_LIMIT),
    }
}

// ── Shaping ───────────────────────────────────────────────────────

/// Remove SGR colour sequences (`ESC [ … m`) from a log line. The
/// server's fmt layer writes colour when stdout looks like a tty, and
/// the container runtime often makes it look like one.
#[must_use]
pub fn strip_ansi(line: &str) -> String {
    static ANSI: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\x1b\[[0-9;]*m").expect("ansi regex"));
    ANSI.replace_all(line, "").into_owned()
}

/// Nanoseconds-since-epoch (as Tempo/Loki write it: a decimal string
/// or a number) to RFC 3339 with millisecond precision.
fn nanos_to_rfc3339(v: &Value) -> Option<String> {
    let ns: i64 = match v {
        Value::String(s) => s.parse().ok()?,
        Value::Number(n) => n.as_i64()?,
        _ => return None,
    };
    let secs = ns.div_euclid(1_000_000_000);
    let sub = u32::try_from(ns.rem_euclid(1_000_000_000)).ok()?;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, sub)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn nanos_i64(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

/// One OTLP `AnyValue` as a display string.
fn any_value_string(v: &Value) -> String {
    if let Some(s) = v.get("stringValue").and_then(Value::as_str) {
        return s.to_owned();
    }
    for key in ["intValue", "doubleValue", "boolValue"] {
        if let Some(x) = v.get(key) {
            return match x {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
        }
    }
    if let Some(arr) = v
        .get("arrayValue")
        .and_then(|a| a.get("values"))
        .and_then(Value::as_array)
    {
        let parts: Vec<String> = arr.iter().map(any_value_string).collect();
        return format!("[{}]", parts.join(", "));
    }
    v.to_string()
}

/// OTLP `[{key, value}]` → `{"key": "value"}`, values stringified.
fn flatten_attributes(attrs: Option<&Value>) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    if let Some(list) = attrs.and_then(Value::as_array) {
        for kv in list {
            if let Some(k) = kv.get("key").and_then(Value::as_str) {
                let v = kv.get("value").map_or_else(String::new, any_value_string);
                out.insert(k.to_owned(), Value::String(v));
            }
        }
    }
    out
}

/// Tempo `GET /api/search` → `[{trace_id, root_service, root_name, start, duration_ms, span_count}]`.
#[must_use]
pub fn shape_search(body: &Value) -> Vec<Value> {
    let Some(traces) = body.get("traces").and_then(Value::as_array) else {
        return Vec::new();
    };
    traces
        .iter()
        .map(|t| {
            let span_count = t
                .get("spanSet")
                .and_then(|s| s.get("matched"))
                .and_then(Value::as_u64)
                .or_else(|| {
                    t.get("spanSets")
                        .and_then(Value::as_array)
                        .and_then(|s| s.first())
                        .and_then(|s| s.get("matched"))
                        .and_then(Value::as_u64)
                })
                .or_else(|| {
                    t.get("spanSet")
                        .and_then(|s| s.get("spans"))
                        .and_then(Value::as_array)
                        .map(|s| s.len() as u64)
                });
            json!({
                "trace_id": t.get("traceID").cloned().unwrap_or(Value::Null),
                "root_service": t.get("rootServiceName").cloned().unwrap_or(Value::Null),
                "root_name": t.get("rootTraceName").cloned().unwrap_or(Value::Null),
                "start": t.get("startTimeUnixNano").and_then(nanos_to_rfc3339),
                "duration_ms": t.get("durationMs").cloned().unwrap_or(Value::Null),
                "span_count": span_count,
            })
        })
        .collect()
}

/// Tempo `GET /api/traces/<id>` (OTLP JSON) → spans sorted by start.
///
/// Accepts both the `batches` key older Tempo writes and the
/// `resourceSpans` key of newer releases.
#[must_use]
pub fn shape_trace(body: &Value) -> Vec<Value> {
    let batches = body
        .get("batches")
        .or_else(|| body.get("resourceSpans"))
        .and_then(Value::as_array);
    let Some(batches) = batches else {
        return Vec::new();
    };
    let mut spans: Vec<(i64, Value)> = Vec::new();
    for batch in batches {
        let resource = flatten_attributes(batch.get("resource").and_then(|r| r.get("attributes")));
        let service = resource
            .get("service.name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let scopes = batch
            .get("scopeSpans")
            .or_else(|| batch.get("instrumentationLibrarySpans"))
            .and_then(Value::as_array);
        for scope in scopes.into_iter().flatten() {
            for span in scope
                .get("spans")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let start = span
                    .get("startTimeUnixNano")
                    .and_then(nanos_i64)
                    .unwrap_or(0);
                let end = span
                    .get("endTimeUnixNano")
                    .and_then(nanos_i64)
                    .unwrap_or(start);
                let duration_ms = (end - start) as f64 / 1_000_000.0;
                let status = match span.get("status").and_then(|s| s.get("code")) {
                    Some(Value::Number(n)) => match n.as_i64() {
                        Some(1) => "ok",
                        Some(2) => "error",
                        _ => "unset",
                    },
                    Some(Value::String(s)) => match s.as_str() {
                        "STATUS_CODE_OK" => "ok",
                        "STATUS_CODE_ERROR" => "error",
                        _ => "unset",
                    },
                    _ => "unset",
                };
                let mut row = json!({
                    "span_id": span.get("spanId").cloned().unwrap_or(Value::Null),
                    "parent": span
                        .get("parentSpanId")
                        .and_then(Value::as_str)
                        .filter(|p| !p.is_empty()),
                    "service": service,
                    "name": span.get("name").cloned().unwrap_or(Value::Null),
                    "start": nanos_to_rfc3339(&json!(start)),
                    "duration_ms": (duration_ms * 1000.0).round() / 1000.0,
                    "status": status,
                    "attributes": Value::Object(flatten_attributes(span.get("attributes"))),
                });
                if let Some(msg) = span
                    .get("status")
                    .and_then(|s| s.get("message"))
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty())
                {
                    row["status_message"] = json!(msg);
                }
                spans.push((start, row));
            }
        }
    }
    spans.sort_by_key(|(start, _)| *start);
    spans.into_iter().map(|(_, row)| row).collect()
}

/// Loki `GET /loki/api/v1/query_range` → `[{ts, labels, line}]`, newest first.
#[must_use]
pub fn shape_logs(body: &Value) -> Vec<Value> {
    let Some(streams) = body
        .get("data")
        .and_then(|d| d.get("result"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut rows: Vec<(i64, Value)> = Vec::new();
    for stream in streams {
        let labels = stream.get("stream").cloned().unwrap_or_else(|| json!({}));
        for entry in stream
            .get("values")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(pair) = entry.as_array() else {
                continue;
            };
            let (Some(ts), Some(line)) = (pair.first(), pair.get(1).and_then(Value::as_str)) else {
                continue;
            };
            let ns = nanos_i64(ts).unwrap_or(0);
            rows.push((
                ns,
                json!({
                    "ts": nanos_to_rfc3339(ts),
                    "labels": labels,
                    "line": strip_ansi(line),
                }),
            ));
        }
    }
    rows.sort_by_key(|(ns, _)| std::cmp::Reverse(*ns));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// Keep rows while their compact serialization fits under `cap`.
/// Returns the kept rows and whether any were dropped.
#[must_use]
pub fn cap_rows(rows: Vec<Value>, cap: usize) -> (Vec<Value>, bool) {
    let total = rows.len();
    let mut used = 0usize;
    let mut kept = Vec::with_capacity(total);
    for row in rows {
        let size = serde_json::to_string(&row).map_or(0, |s| s.len() + 1);
        if used + size > cap && !kept.is_empty() {
            break;
        }
        used += size;
        kept.push(row);
    }
    let truncated = kept.len() < total;
    (kept, truncated)
}

/// Wrap shaped rows in the envelope every tool returns.
fn envelope(kind: &str, rows: Vec<Value>, extra: Value) -> Value {
    let (rows, truncated) = cap_rows(rows, OUTPUT_CAP_BYTES);
    let mut out = json!({
        "count": rows.len(),
        "truncated": truncated,
        kind: rows,
    });
    if let (Some(dst), Some(src)) = (out.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    out
}

// ── Client ────────────────────────────────────────────────────────

/// The read-side client. Cheap to build per call; `reqwest::Client`
/// pools connections internally.
pub struct TelemetryClient {
    http: reqwest::Client,
    cfg: TelemetryConfig,
}

impl TelemetryClient {
    #[must_use]
    pub fn new(cfg: TelemetryConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_default();
        Self { http, cfg }
    }

    #[must_use]
    pub fn config(&self) -> &TelemetryConfig {
        &self.cfg
    }

    fn tempo(&self) -> Result<&str, QueryError> {
        self.cfg
            .tempo
            .as_deref()
            .ok_or(QueryError::Unconfigured(TEMPO_URL_VAR))
    }

    fn loki(&self) -> Result<&str, QueryError> {
        self.cfg
            .loki
            .as_deref()
            .ok_or(QueryError::Unconfigured(LOKI_URL_VAR))
    }

    async fn get_json(&self, url: &str, query: &[(&str, String)]) -> Result<Value, QueryError> {
        let res = self
            .http
            .get(url)
            .query(query)
            .send()
            .await
            .map_err(|e| QueryError::Upstream(format!("GET {url}: {e}")))?;
        let status = res.status();
        let text = res
            .text()
            .await
            .map_err(|e| QueryError::Upstream(format!("GET {url}: reading body: {e}")))?;
        if !status.is_success() {
            let snippet: String = text.chars().take(400).collect();
            return Err(QueryError::Upstream(format!(
                "{url} answered {status}: {snippet}"
            )));
        }
        serde_json::from_str(&text)
            .map_err(|e| QueryError::Upstream(format!("{url}: body is not JSON ({e})")))
    }

    /// TraceQL search over a lookback window.
    pub async fn search_traces(
        &self,
        traceql: &str,
        since: &str,
        limit: usize,
    ) -> Result<Value, QueryError> {
        let base = self.tempo()?;
        let q = traceql.trim();
        if q.is_empty() {
            return Err(QueryError::BadRequest("`traceql` is required".into()));
        }
        let (start, end) = window_secs(parse_since(since)?);
        let body = self
            .get_json(
                &format!("{base}/api/search"),
                &[
                    ("q", q.to_owned()),
                    ("start", start.to_string()),
                    ("end", end.to_string()),
                    ("limit", limit.to_string()),
                ],
            )
            .await?;
        Ok(envelope(
            "traces",
            shape_search(&body),
            json!({ "query": q, "since": since, "note": "Pass a trace_id to telemetry_get_trace for its spans." }),
        ))
    }

    /// One trace by id, spans flattened and sorted by start.
    pub async fn get_trace(&self, trace_id: &str) -> Result<Value, QueryError> {
        let base = self.tempo()?;
        let id = trace_id.trim();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(QueryError::BadRequest(format!(
                "`trace_id` must be a hex trace id (got `{id}`)"
            )));
        }
        let body = self
            .get_json(&format!("{base}/api/traces/{id}"), &[])
            .await?;
        let spans = shape_trace(&body);
        if spans.is_empty() {
            return Err(QueryError::Upstream(format!(
                "trace `{id}` has no spans (not found, or outside Tempo's retention)"
            )));
        }
        Ok(envelope("spans", spans, json!({ "trace_id": id })))
    }

    /// LogQL range query over a lookback window, newest first.
    pub async fn query_logs(
        &self,
        logql: &str,
        since: &str,
        limit: usize,
    ) -> Result<Value, QueryError> {
        let base = self.loki()?;
        let q = logql.trim();
        if q.is_empty() {
            return Err(QueryError::BadRequest("`logql` is required".into()));
        }
        let (start, end) = window_secs(parse_since(since)?);
        let body = self
            .get_json(
                &format!("{base}/loki/api/v1/query_range"),
                &[
                    ("query", q.to_owned()),
                    ("start", format!("{start}000000000")),
                    ("end", format!("{end}000000000")),
                    ("limit", limit.to_string()),
                    ("direction", "backward".to_owned()),
                ],
            )
            .await?;
        let mut rows = shape_logs(&body);
        rows.truncate(limit);
        Ok(envelope(
            "logs",
            rows,
            json!({ "query": q, "since": since }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_parses_the_common_units_and_defaults_to_minutes() {
        assert_eq!(parse_since("15m").unwrap(), Duration::from_secs(900));
        assert_eq!(parse_since("2h").unwrap(), Duration::from_secs(7_200));
        assert_eq!(parse_since("1d").unwrap(), Duration::from_secs(86_400));
        assert_eq!(parse_since("90s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_since(" 5 ").unwrap(), Duration::from_secs(300));
        assert_eq!(
            parse_since(DEFAULT_SINCE).unwrap(),
            Duration::from_secs(3_600)
        );
    }

    #[test]
    fn since_rejects_garbage_zero_and_huge() {
        for bad in ["", "abc", "0m", "5x", "-3h", "31d"] {
            let err = parse_since(bad).expect_err(bad);
            assert!(matches!(err, QueryError::BadRequest(_)), "{bad}: {err:?}");
        }
    }

    #[test]
    fn ansi_colour_is_stripped_but_text_kept() {
        let line = "\x1b[2m2026-09-01T10:00:00Z\x1b[0m \x1b[32m INFO\x1b[0m listening \x1b[1;31mbind\x1b[0m=0.0.0.0:8080";
        assert_eq!(
            strip_ansi(line),
            "2026-09-01T10:00:00Z  INFO listening bind=0.0.0.0:8080"
        );
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn userinfo_is_redacted_from_urls() {
        assert_eq!(
            redact_userinfo("http://user:s3cret@tempo.svc:3200/x"),
            "http://tempo.svc:3200/x"
        );
        assert_eq!(
            redact_userinfo("http://tempo.svc:3200"),
            "http://tempo.svc:3200"
        );
        assert_eq!(redact_userinfo("not a url"), "not a url");
    }

    #[test]
    fn tempo_search_shapes_to_one_row_per_trace() {
        let body = json!({
            "traces": [{
                "traceID": "abc123",
                "rootServiceName": "task-server",
                "rootTraceName": "TimerService/list_sessions",
                "startTimeUnixNano": "1756720800000000000",
                "durationMs": 512,
                "spanSet": { "matched": 3, "spans": [{}, {}, {}] }
            }]
        });
        let rows = shape_search(&body);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r["trace_id"], "abc123");
        assert_eq!(r["root_service"], "task-server");
        assert_eq!(r["root_name"], "TimerService/list_sessions");
        assert_eq!(r["duration_ms"], 512);
        assert_eq!(r["span_count"], 3);
        assert_eq!(r["start"], "2025-09-01T10:00:00.000Z");
        assert!(shape_search(&json!({})).is_empty());
    }

    #[test]
    fn tempo_trace_flattens_attributes_and_sorts_by_start() {
        let body = json!({
            "batches": [{
                "resource": { "attributes": [
                    { "key": "service.name", "value": { "stringValue": "task-server" } }
                ]},
                "scopeSpans": [{ "spans": [
                    {
                        "spanId": "child", "parentSpanId": "root", "name": "auth.resolve",
                        "startTimeUnixNano": "1756720800002000000",
                        "endTimeUnixNano":   "1756720800003500000",
                        "attributes": [
                            { "key": "auth.outcome", "value": { "stringValue": "rejected" } },
                            { "key": "auth.token_presented", "value": { "boolValue": true } },
                            { "key": "http.status", "value": { "intValue": "401" } }
                        ],
                        "status": { "code": 2, "message": "bad token" }
                    },
                    {
                        "spanId": "root", "parentSpanId": "", "name": "http.request",
                        "startTimeUnixNano": "1756720800000000000",
                        "endTimeUnixNano":   "1756720800010000000",
                        "attributes": [],
                        "status": { "code": 0 }
                    }
                ]}]
            }]
        });
        let spans = shape_trace(&body);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0]["span_id"], "root", "sorted by start");
        assert_eq!(spans[0]["parent"], Value::Null, "empty parent becomes null");
        assert_eq!(spans[0]["duration_ms"], 10.0);
        assert_eq!(spans[0]["status"], "unset");
        let child = &spans[1];
        assert_eq!(child["service"], "task-server");
        assert_eq!(child["duration_ms"], 1.5);
        assert_eq!(child["status"], "error");
        assert_eq!(child["status_message"], "bad token");
        assert_eq!(child["attributes"]["auth.outcome"], "rejected");
        assert_eq!(child["attributes"]["auth.token_presented"], "true");
        assert_eq!(child["attributes"]["http.status"], "401");

        // Newer Tempo: same body under `resourceSpans`.
        let newer = json!({ "resourceSpans": body["batches"].clone() });
        assert_eq!(shape_trace(&newer).len(), 2);
    }

    #[test]
    fn loki_rows_carry_labels_and_clean_lines_newest_first() {
        let body = json!({
            "data": { "result": [
                {
                    "stream": { "namespace": "task", "container": "task-server" },
                    "values": [
                        ["1756720800000000000", "\x1b[32mINFO\x1b[0m central auth: issuer_unreachable"],
                        ["1756720803000000000", "later line"]
                    ]
                },
                {
                    "stream": { "namespace": "task", "container": "task-web" },
                    "values": [["1756720801000000000", "middle line"]]
                }
            ]}
        });
        let rows = shape_logs(&body);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["line"], "later line");
        assert_eq!(rows[1]["line"], "middle line");
        assert_eq!(rows[1]["labels"]["container"], "task-web");
        assert_eq!(rows[2]["line"], "INFO central auth: issuer_unreachable");
        assert_eq!(rows[2]["ts"], "2025-09-01T10:00:00.000Z");
        assert!(shape_logs(&json!({ "status": "success" })).is_empty());
    }

    #[test]
    fn cap_keeps_rows_until_the_budget_and_flags_the_cut() {
        let rows: Vec<Value> = (0..100)
            .map(|i| json!({ "i": i, "pad": "x".repeat(50) }))
            .collect();
        let (kept, truncated) = cap_rows(rows.clone(), 1_000);
        assert!(truncated);
        assert!(!kept.is_empty() && kept.len() < 100, "{}", kept.len());
        let (all, truncated) = cap_rows(rows, OUTPUT_CAP_BYTES);
        assert!(!truncated);
        assert_eq!(all.len(), 100);
        // One oversized row is still returned, so a huge span is
        // readable rather than silently absent.
        let (one, _) = cap_rows(vec![json!({ "big": "y".repeat(5_000) })], 100);
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn limits_clamp_and_default() {
        assert_eq!(clamp_limit(None), DEFAULT_LIMIT);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(7)), 7);
        assert_eq!(clamp_limit(Some(10_000)), MAX_LIMIT);
    }

    #[test]
    fn config_reports_presence_without_secrets() {
        let cfg = TelemetryConfig {
            tempo: Some("http://op:pw@tempo:3200".into()),
            loki: None,
        };
        assert!(cfg.any());
        let d = cfg.describe();
        assert_eq!(d["tempo"], "http://tempo:3200");
        assert_eq!(d["loki"], Value::Null);
        assert!(!TelemetryConfig::default().any());
    }
}
