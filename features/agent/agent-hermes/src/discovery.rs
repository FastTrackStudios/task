//! Discovery — proxied from the gateway's REST surface
//! (`/v1/models`, `/v1/skills`, `/v1/capabilities`).
//!
//! The trait is sync (Facet RPC); calls run on the dispatcher's
//! blocking threads, so the async HTTP hops are driven via the
//! runtime handle captured at construction. Responses are parsed
//! defensively — the gateway's exact JSON evolves between Hermes
//! releases, and a missing field must degrade to an empty label,
//! not an error.

use agent_proto::error::AgentError;
use agent_proto::service::discovery::{
    BackendHealth, CapabilityFlag, Discovery, ModelInfo, SkillInfo,
};
use serde_json::Value;

use crate::{BACKEND_ID, HermesBackend};

impl HermesBackend {
    /// Blocking GET against the gateway, JSON-decoded.
    fn gateway_get(&self, path: &str) -> Result<Value, AgentError> {
        let url = format!("{}{path}", self.inner.config.base_url);
        let http = self.inner.http.clone();
        let key = self.inner.config.api_key.clone();
        self.inner
            .runtime
            .block_on(async move {
                let mut req = http.get(&url);
                if !key.is_empty() {
                    req = req.header("Authorization", format!("Bearer {key}"));
                }
                let resp = req.send().await.map_err(|e| e.to_string())?;
                let status = resp.status();
                if !status.is_success() {
                    return Err(format!("HTTP {status}"));
                }
                resp.json::<Value>().await.map_err(|e| e.to_string())
            })
            .map_err(|e| AgentError::Io(format!("hermes {path}: {e}")))
    }
}

/// The gateway wraps lists differently per endpoint — accept
/// `{"data": [...]}`, `{"skills": [...]}`, or a bare array.
fn rows<'a>(v: &'a Value, keys: &[&str]) -> Vec<&'a Value> {
    if let Some(arr) = v.as_array() {
        return arr.iter().collect();
    }
    for k in keys {
        if let Some(arr) = v.get(*k).and_then(Value::as_array) {
            return arr.iter().collect();
        }
    }
    Vec::new()
}

fn s(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// models.dev provider ids surfaced in the picker when
/// `TASK_HERMES_PROVIDERS` doesn't override — the set the gateway
/// deployment can plausibly route to (its `/model` chat command
/// switches providers per session).
const DEFAULT_PROVIDERS: &str =
    "openai,anthropic,google,github-copilot,deepseek,x-ai,qwen,nousresearch";

/// In-process models.dev catalog cache (1h TTL) — the catalog is
/// ~2MB of JSON and changes rarely.
static CATALOG: std::sync::Mutex<Option<(std::time::Instant, Value)>> = std::sync::Mutex::new(None);

impl HermesBackend {
    /// The models.dev catalog (fetched + cached). The same source
    /// hermes-agent's own CLI uses for its `/model` picker; the
    /// gateway exposes no catalog endpoint, so we go straight to it.
    fn models_dev_catalog(&self) -> Option<Value> {
        {
            let cache = CATALOG.lock().ok()?;
            if let Some((at, v)) = cache.as_ref() {
                if at.elapsed() < std::time::Duration::from_secs(3600) {
                    return Some(v.clone());
                }
            }
        }
        let http = self.inner.http.clone();
        let fetched = self
            .inner
            .runtime
            .block_on(async move {
                let resp = http
                    .get("https://models.dev/api.json")
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                if !resp.status().is_success() {
                    return Err(format!("HTTP {}", resp.status()));
                }
                resp.json::<Value>().await.map_err(|e| e.to_string())
            })
            .ok()?;
        if let Ok(mut cache) = CATALOG.lock() {
            *cache = Some((std::time::Instant::now(), fetched.clone()));
        }
        Some(fetched)
    }
}

/// Catalog pricing for `provider/model`, read from the cache only —
/// callable from async contexts (the streaming turn worker) where
/// the blocking fetch would deadlock. `None` until the picker has
/// warmed the cache, or for models the catalog doesn't know.
pub(crate) fn cached_price(model_id: &str) -> Option<(f64, f64)> {
    let (provider, model) = model_id.split_once('/')?;
    let cache = CATALOG.lock().ok()?;
    let (_, catalog) = cache.as_ref()?;
    let m = catalog.pointer(&format!("/{provider}/models/{model}"))?;
    let cin = m
        .pointer("/cost/input")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let cout = m
        .pointer("/cost/output")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    (cin > 0.0 || cout > 0.0).then_some((cin, cout))
}

/// Names of the platform adapters the gateway reports as connected.
/// `/health/detailed` returns `{"platforms": {"discord": true}}` or
/// `{"platforms": {"discord": {"connected": true}}}` depending on
/// the release — accept both, and treat a bare present key as
/// connected when neither shape carries a boolean.
fn connected_platforms(v: &Value) -> Vec<String> {
    let Some(map) = v.get("platforms").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out: Vec<String> = map
        .iter()
        .filter(|(_, state)| match state {
            Value::Bool(b) => *b,
            Value::Object(o) => o
                .get("connected")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| o.get("state").and_then(Value::as_str) != Some("disconnected")),
            _ => true,
        })
        .map(|(name, _)| name.clone())
        .collect();
    out.sort();
    out
}

/// A one-line note about anything unhealthy in the gateway's
/// `readiness` block (`state_db`, `config`, `model`, `disk`, …).
/// Empty when everything reports ok — the chip stays quiet unless
/// there's something to say.
fn readiness_note(v: &Value) -> String {
    let Some(checks) = v.pointer("/readiness/checks").and_then(Value::as_object) else {
        return String::new();
    };
    let bad: Vec<String> = checks
        .iter()
        .filter_map(|(name, c)| {
            let status = c.get("status").and_then(Value::as_str).unwrap_or("ok");
            (status != "ok").then(|| format!("{name}: {status}"))
        })
        .collect();
    if bad.is_empty() {
        return String::new();
    }
    format!("degraded — {}", bad.join(", "))
}

/// Agent runs the gateway currently has in flight. Newer releases
/// report `active_agents` at the top level; v0.19 only counts them
/// under the readiness block's background-queue check.
fn in_flight_agents(v: &Value) -> u32 {
    v.get("active_agents")
        .and_then(Value::as_u64)
        .or_else(|| {
            v.pointer("/readiness/checks/background_queues/active_api_runs")
                .and_then(Value::as_u64)
        })
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}

impl HermesBackend {
    /// `/health/detailed` lives at the server root, not under the
    /// versioned API base the rest of the client uses.
    fn health_url(&self) -> String {
        let root = self.inner.config.base_url.trim_end_matches("/v1");
        format!("{root}/health/detailed")
    }
}

impl Discovery for HermesBackend {
    fn list_models(&self, _backend_id: &str) -> Result<Vec<ModelInfo>, AgentError> {
        let default = self.inner.config.model.clone();
        let fallback_ctx = std::env::var("TASK_HERMES_CONTEXT_LENGTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1_050_000);

        // The gateway's own façade model first — "the agent as
        // configured", always present and the default.
        let mut out = vec![ModelInfo {
            backend_id: BACKEND_ID.to_string(),
            id: default.clone(),
            label: "Hermes (configured default)".to_string(),
            is_default: true,
            context_length: fallback_ctx,
            provider_id: "hermes".to_string(),
            provider_name: "Hermes Gateway".to_string(),
            reasoning: true,
            cost_in_per_mtok: 0.0,
            cost_out_per_mtok: 0.0,
        }];

        // Provider-grouped catalog from models.dev, scoped to the
        // providers the deployment can route to. Selecting one of
        // these switches the session via the `/model` chat command.
        if let Some(catalog) = self.models_dev_catalog() {
            let scoped: Vec<String> = std::env::var("TASK_HERMES_PROVIDERS")
                .unwrap_or_else(|_| DEFAULT_PROVIDERS.to_string())
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect();
            for pid in &scoped {
                let Some(provider) = catalog.get(pid) else {
                    continue;
                };
                let pname = {
                    let n = s(provider, "name");
                    if n.is_empty() { pid.clone() } else { n }
                };
                let Some(models) = provider.get("models").and_then(Value::as_object) else {
                    continue;
                };
                for (mid, m) in models {
                    // Chat-capable only: the agent needs tool calling.
                    if !m.get("tool_call").and_then(Value::as_bool).unwrap_or(false) {
                        continue;
                    }
                    out.push(ModelInfo {
                        backend_id: BACKEND_ID.to_string(),
                        id: format!("{pid}/{mid}"),
                        label: s(m, "name"),
                        is_default: false,
                        context_length: m
                            .pointer("/limit/context")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        provider_id: pid.clone(),
                        provider_name: pname.clone(),
                        reasoning: m.get("reasoning").and_then(Value::as_bool).unwrap_or(false),
                        cost_in_per_mtok: m
                            .pointer("/cost/input")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0),
                        cost_out_per_mtok: m
                            .pointer("/cost/output")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0),
                    });
                }
            }
        }
        Ok(out)
    }

    fn list_skills(&self, _backend_id: &str) -> Result<Vec<SkillInfo>, AgentError> {
        let v = self.gateway_get("/skills")?;
        Ok(rows(&v, &["data", "skills"])
            .into_iter()
            .filter_map(|sk| {
                let name = if sk.is_string() {
                    sk.as_str().unwrap_or_default().to_string()
                } else {
                    let n = s(sk, "name");
                    if n.is_empty() { s(sk, "id") } else { n }
                };
                if name.is_empty() {
                    return None;
                }
                Some(SkillInfo {
                    backend_id: BACKEND_ID.to_string(),
                    description: s(sk, "description"),
                    enabled: sk.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                    name,
                })
            })
            .collect())
    }

    fn backend_health(&self, _backend_id: &str) -> Result<Vec<BackendHealth>, AgentError> {
        let url = self.health_url();
        let http = self.inner.http.clone();
        let key = self.inner.config.api_key.clone();
        let started = std::time::Instant::now();
        // Whether `/health/detailed` is auth-gated varies by release
        // (newer ones serve it open for cross-container dashboards,
        // deployed v2026.7.20 answers `invalid_api_key` without a
        // bearer), so always send the token when we have one.
        let probed: Result<Value, String> = self.inner.runtime.block_on(async move {
            let mut req = http.get(&url).timeout(std::time::Duration::from_secs(5));
            if !key.is_empty() {
                req = req.header("Authorization", format!("Bearer {key}"));
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                return Err(format!("HTTP {status}"));
            }
            resp.json::<Value>().await.map_err(|e| e.to_string())
        });
        let latency_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);

        let at = chrono::Utc::now();
        Ok(vec![match probed {
            Ok(v) => BackendHealth {
                backend_id: BACKEND_ID.to_string(),
                reachable: true,
                last_ping_ms: latency_ms,
                version: s(&v, "version"),
                // `exit_reason` is set when the gateway is winding
                // down; otherwise report whatever readiness check is
                // unhappy. Both are worth surfacing, neither is
                // usually present.
                status_text: {
                    let exit = s(&v, "exit_reason");
                    if exit.is_empty() {
                        readiness_note(&v)
                    } else {
                        exit
                    }
                },
                state: s(&v, "gateway_state"),
                active_agents: in_flight_agents(&v),
                platforms: connected_platforms(&v),
                model: self.inner.config.model.clone(),
                at,
            },
            Err(e) => BackendHealth {
                backend_id: BACKEND_ID.to_string(),
                reachable: false,
                last_ping_ms: latency_ms,
                version: String::new(),
                status_text: format!("{}: {e}", self.inner.config.base_url),
                state: String::new(),
                active_agents: 0,
                platforms: Vec::new(),
                model: self.inner.config.model.clone(),
                at,
            },
        }])
    }

    fn list_capabilities(&self, _backend_id: &str) -> Result<Vec<CapabilityFlag>, AgentError> {
        let v = self.gateway_get("/capabilities")?;
        // Flatten one level of {group: {flag: bool}} plus top-level bools.
        let mut out = Vec::new();
        let obj = v
            .get("capabilities")
            .and_then(Value::as_object)
            .or_else(|| v.as_object());
        if let Some(map) = obj {
            for (k, val) in map {
                match val {
                    Value::Bool(b) => out.push(CapabilityFlag {
                        backend_id: BACKEND_ID.to_string(),
                        name: k.clone(),
                        enabled: *b,
                    }),
                    Value::Object(inner) => {
                        for (ik, iv) in inner {
                            if let Some(b) = iv.as_bool() {
                                out.push(CapabilityFlag {
                                    backend_id: BACKEND_ID.to_string(),
                                    name: format!("{k}.{ik}"),
                                    enabled: b,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn platforms_accept_both_shapes_and_skip_disconnected() {
        let v = json!({
            "platforms": {
                "discord": true,
                "slack": false,
                "api_server": {"connected": true},
                "matrix": {"connected": false},
                "cron": {"state": "disconnected"},
                "mirror": {},
            }
        });
        assert_eq!(
            connected_platforms(&v),
            vec!["api_server".to_string(), "discord".into(), "mirror".into()]
        );
    }

    #[test]
    fn platforms_missing_key_is_empty() {
        assert!(connected_platforms(&json!({"status": "ok"})).is_empty());
    }

    /// Trimmed from a live `GET /health/detailed` against the
    /// deployed gateway (hermes-agent 0.19.0).
    fn live_payload() -> Value {
        json!({
            "status": "ok",
            "readiness": {
                "status": "ok",
                "checks": {
                    "state_db": {"status": "ok"},
                    "config": {"status": "ok"},
                    "model": {"status": "ok"},
                    "disk": {"status": "ok", "used_percent": 71.9},
                    "gateway": {"status": "ok", "state": "running", "connected_platforms": 2},
                    "background_queues": {"status": "ok", "active_api_runs": 2}
                }
            },
            "platform": "hermes-agent",
            "version": "0.19.0",
            "gateway_state": "running",
            "platforms": {
                "webhook": {"state": "connected"},
                "api_server": {"state": "connected"},
                "nextcloud_talk": {"state": "disconnected"}
            }
        })
    }

    #[test]
    fn live_health_payload_parses() {
        let v = live_payload();
        assert_eq!(s(&v, "version"), "0.19.0");
        assert_eq!(s(&v, "gateway_state"), "running");
        assert_eq!(
            connected_platforms(&v),
            vec!["api_server".to_string(), "webhook".into()]
        );
        // v0.19 has no top-level `active_agents` — read the queue check.
        assert_eq!(in_flight_agents(&v), 2);
        assert_eq!(readiness_note(&v), "");
    }

    #[test]
    fn readiness_note_names_the_unhappy_checks() {
        let mut v = live_payload();
        v["readiness"]["checks"]["model"]["status"] = json!("error");
        assert_eq!(readiness_note(&v), "degraded — model: error");
        // Nothing to report when the block is absent entirely.
        assert_eq!(readiness_note(&json!({"status": "ok"})), "");
    }

    #[test]
    fn in_flight_prefers_the_top_level_count() {
        let mut v = live_payload();
        v["active_agents"] = json!(7);
        assert_eq!(in_flight_agents(&v), 7);
    }

    #[test]
    fn cached_price_needs_a_provider_qualified_id() {
        // Bare ids never resolve — the catalog is keyed provider/model.
        assert_eq!(cached_price("hermes"), None);
    }
}
