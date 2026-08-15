//! Routines — the gateway's cron jobs (`/api/jobs`), mapped onto
//! [`agent_proto::service::routines`].
//!
//! The gateway's job record is wider than what a user schedules and
//! watches: per-job provider/base-url overrides, script hooks, and
//! an `origin` blob describing which chat platform created it. We
//! project the parts a person acts on and leave the rest as
//! gateway-side config.
//!
//! Schedules round-trip asymmetrically on purpose. Callers *send* an
//! expression the gateway parses (`"every 2h"`, `"0 9 * * *"`,
//! `"30m"`); the gateway *returns* a parsed object plus a rendered
//! `schedule_display`, and that display string is what we surface —
//! it's what the user recognizes.

use agent_proto::error::AgentError;
use agent_proto::service::routines::{NewRoutine, Routine, Routines};
use serde_json::{Value, json};

use crate::{BACKEND_ID, HermesBackend};

/// The gateway gates jobs behind an optional `cron` dependency and
/// answers 503 when it's missing. That's "no routines here", not a
/// failure the UI should shout about.
const UNAVAILABLE: u16 = 503;

impl HermesBackend {
    /// Blocking request against the gateway's non-versioned `/api`
    /// surface. `body` is sent when present; `None` = a bare GET or
    /// DELETE depending on `method`.
    fn api_call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, AgentError> {
        // `/api/jobs` sits at the server root, not under `/v1`.
        let root = self.inner.config.base_url.trim_end_matches("/v1");
        let url = format!("{root}{path}");
        let http = self.inner.http.clone();
        let key = self.inner.config.api_key.clone();
        let method = method.to_string();
        self.inner
            .runtime
            .block_on(async move {
                let mut req = match method.as_str() {
                    "POST" => http.post(&url),
                    "PATCH" => http.patch(&url),
                    "DELETE" => http.delete(&url),
                    _ => http.get(&url),
                };
                if !key.is_empty() {
                    req = req.header("Authorization", format!("Bearer {key}"));
                }
                if let Some(b) = body {
                    req = req.json(&b);
                }
                let resp = req
                    .timeout(std::time::Duration::from_secs(15))
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                let status = resp.status();
                if status.as_u16() == UNAVAILABLE {
                    return Err("scheduler unavailable".to_string());
                }
                if !status.is_success() {
                    let detail = resp.text().await.unwrap_or_default();
                    return Err(format!("HTTP {status}: {detail}"));
                }
                // DELETE may answer with an empty body.
                let text = resp.text().await.map_err(|e| e.to_string())?;
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                serde_json::from_str::<Value>(&text).map_err(|e| e.to_string())
            })
            .map_err(|e| AgentError::Io(format!("hermes {path}: {e}")))
    }
}

fn s(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Project one gateway job onto a [`Routine`].
pub(crate) fn routine_from_job(job: &Value) -> Routine {
    // `schedule` is an object (`{kind, expr|minutes|run_at, display}`)
    // with a sibling `schedule_display`. Prefer the sibling, fall back
    // to the nested display, then to the raw expression.
    let schedule = {
        let display = s(job, "schedule_display");
        if !display.is_empty() {
            display
        } else {
            let nested = job.get("schedule");
            nested
                .map(|sc| {
                    let d = s(sc, "display");
                    if d.is_empty() { s(sc, "expr") } else { d }
                })
                .unwrap_or_default()
        }
    };
    Routine {
        backend_id: BACKEND_ID.to_string(),
        id: s(job, "id"),
        name: s(job, "name"),
        prompt: s(job, "prompt"),
        schedule,
        kind: job
            .pointer("/schedule/kind")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        enabled: job.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        state: s(job, "state"),
        next_run_at: s(job, "next_run_at"),
        last_run_at: s(job, "last_run_at"),
        last_status: s(job, "last_status"),
        // Surface whichever failure the gateway recorded — a job that
        // ran fine but couldn't deliver is still broken from here.
        last_error: {
            let run = s(job, "last_error");
            if run.is_empty() {
                s(job, "last_delivery_error")
            } else {
                run
            }
        },
        deliver: s(job, "deliver"),
        runs_completed: job
            .pointer("/repeat/completed")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0),
        // `times: null` means forever, which we carry as 0.
        runs_total: job
            .pointer("/repeat/times")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0),
        skills: job
            .get("skills")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        // `model` is only set by an explicit per-job override;
        // `model_snapshot` is what the job will actually run with,
        // captured when it was scheduled. Show the effective one.
        model: {
            let explicit = s(job, "model");
            if explicit.is_empty() {
                s(job, "model_snapshot")
            } else {
                explicit
            }
        },
    }
}

/// Jobs come back as `{"jobs": [...]}`; a single mutation answers
/// `{"job": {...}}`. Accept a bare object/array too.
fn jobs_of(v: &Value) -> Vec<Routine> {
    v.get("jobs")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(routine_from_job).collect())
        .or_else(|| {
            v.as_array()
                .map(|a| a.iter().map(routine_from_job).collect())
        })
        .unwrap_or_default()
}

fn job_of(v: &Value) -> Result<Routine, AgentError> {
    let job = v.get("job").unwrap_or(v);
    if job.get("id").is_none() {
        return Err(AgentError::Io("hermes: no job in the response".to_string()));
    }
    Ok(routine_from_job(job))
}

/// The gateway rejects a create with an empty `name` (400 "Name is
/// required"), but a name is genuinely optional to a user writing a
/// one-line routine. Derive one from the prompt the way the
/// gateway's own CLI path does — first 50 characters.
fn derive_name(name: &str, prompt: &str) -> String {
    let name = name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    let head: String = prompt.trim().chars().take(50).collect();
    let head = head.trim().to_string();
    if head.is_empty() {
        "routine".to_string()
    } else {
        head
    }
}

impl Routines for HermesBackend {
    fn list_routines(
        &self,
        _backend_id: &str,
        include_disabled: bool,
    ) -> Result<Vec<Routine>, AgentError> {
        let path = if include_disabled {
            "/api/jobs?include_disabled=true"
        } else {
            "/api/jobs"
        };
        Ok(jobs_of(&self.api_call("GET", path, None)?))
    }

    fn create_routine(&self, routine: NewRoutine) -> Result<Routine, AgentError> {
        let mut body = json!({
            "prompt": routine.prompt,
            "schedule": routine.schedule,
            "name": derive_name(&routine.name, &routine.prompt),
        });
        if !routine.deliver.is_empty() {
            body["deliver"] = json!(routine.deliver);
        }
        if !routine.skills.is_empty() {
            body["skills"] = json!(routine.skills);
        }
        // The gateway treats a missing `repeat` as "forever"; it
        // rejects 0, so only send a real count.
        if routine.repeat > 0 {
            body["repeat"] = json!(routine.repeat);
        }
        job_of(&self.api_call("POST", "/api/jobs", Some(body))?)
    }

    fn set_routine_paused(
        &self,
        _backend_id: &str,
        id: &str,
        paused: bool,
    ) -> Result<Routine, AgentError> {
        let verb = if paused { "pause" } else { "resume" };
        job_of(&self.api_call("POST", &format!("/api/jobs/{id}/{verb}"), None)?)
    }

    fn run_routine(&self, _backend_id: &str, id: &str) -> Result<Routine, AgentError> {
        let answered = self.api_call("POST", &format!("/api/jobs/{id}/run"), None)?;
        // A triggered run may answer with a status envelope rather
        // than the job; re-read so callers always get current state.
        job_of(&answered)
            .or_else(|_| job_of(&self.api_call("GET", &format!("/api/jobs/{id}"), None)?))
    }

    fn delete_routine(&self, _backend_id: &str, id: &str) -> Result<(), AgentError> {
        self.api_call("DELETE", &format!("/api/jobs/{id}"), None)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped after `cron.jobs.create_job`'s record.
    fn job() -> Value {
        json!({
            "id": "a1b2c3",
            "name": "Morning brief",
            "prompt": "Summarize what's due today",
            "skills": ["research"],
            "skill": "research",
            "model": null,
            "schedule": { "kind": "cron", "expr": "0 8 * * *", "display": "0 8 * * *" },
            "schedule_display": "0 8 * * *",
            "repeat": { "times": null, "completed": 3 },
            "enabled": true,
            "state": "scheduled",
            "created_at": "2026-07-01T08:00:00-07:00",
            "next_run_at": "2026-07-25T08:00:00-07:00",
            "last_run_at": "2026-07-24T08:00:00-07:00",
            "last_status": "ok",
            "last_error": null,
            "last_delivery_error": null,
            "deliver": "local",
            "origin": null
        })
    }

    #[test]
    fn maps_the_gateway_job_record() {
        let r = routine_from_job(&job());
        assert_eq!(r.backend_id, "hermes");
        assert_eq!(r.id, "a1b2c3");
        assert_eq!(r.name, "Morning brief");
        assert_eq!(r.schedule, "0 8 * * *");
        assert_eq!(r.kind, "cron");
        assert_eq!(r.runs_completed, 3);
        // `times: null` = forever, carried as 0.
        assert_eq!(r.runs_total, 0);
        assert_eq!(r.skills, vec!["research".to_string()]);
        assert!(r.enabled);
        assert!(r.last_error.is_empty());
    }

    #[test]
    fn delivery_failure_surfaces_as_the_error() {
        let mut j = job();
        j["last_delivery_error"] = json!("telegram: 401");
        // A job that ran but couldn't deliver is still broken.
        assert_eq!(routine_from_job(&j).last_error, "telegram: 401");
        // A run failure wins over a delivery one.
        j["last_error"] = json!("model timeout");
        assert_eq!(routine_from_job(&j).last_error, "model timeout");
    }

    #[test]
    fn schedule_falls_back_through_display_then_expr() {
        let mut j = job();
        j["schedule_display"] = json!("");
        assert_eq!(routine_from_job(&j).schedule, "0 8 * * *");
        j["schedule"] = json!({ "kind": "interval", "expr": "every 30m" });
        assert_eq!(routine_from_job(&j).schedule, "every 30m");
        j["schedule"] = json!(null);
        assert!(routine_from_job(&j).schedule.is_empty());
    }

    #[test]
    fn list_and_single_envelopes_both_decode() {
        assert_eq!(jobs_of(&json!({ "jobs": [job()] })).len(), 1);
        assert_eq!(jobs_of(&json!([job()])).len(), 1);
        assert!(jobs_of(&json!({ "jobs": [] })).is_empty());
        assert_eq!(job_of(&json!({ "job": job() })).expect("job").id, "a1b2c3");
        assert_eq!(job_of(&job()).expect("bare job").id, "a1b2c3");
        assert!(job_of(&json!({ "status": "queued" })).is_err());
    }

    /// Verbatim from a job created against the deployed gateway
    /// (hermes-agent 0.19.0) — the fields the hand-written fixture
    /// above doesn't cover.
    #[test]
    fn live_gateway_record_maps() {
        let live = json!({
            "id": "5bf61cc47b7d",
            "name": "probe routine",
            "prompt": "Say PROBE and stop.",
            "skills": [],
            "skill": null,
            "model": null,
            "provider": null,
            "provider_snapshot": "openai-codex",
            "model_snapshot": "gpt-5.5",
            "schedule": { "kind": "interval", "minutes": 360, "display": "every 360m" },
            "schedule_display": "every 360m",
            "repeat": { "times": null, "completed": 0 },
            "enabled": true,
            "state": "scheduled",
            "created_at": "2026-07-24T21:26:02.923651+00:00",
            "next_run_at": "2026-07-25T03:26:02.935757+00:00",
            "last_run_at": null,
            "last_status": null,
            "deliver": "local",
            "origin": { "platform": "api_server", "chat_id": "api" },
            "latest_execution": null
        });
        let r = routine_from_job(&live);
        assert_eq!(r.id, "5bf61cc47b7d");
        assert_eq!(r.schedule, "every 360m");
        assert_eq!(r.kind, "interval");
        assert_eq!(r.state, "scheduled");
        assert_eq!(r.next_run_at, "2026-07-25T03:26:02.935757+00:00");
        assert!(r.last_run_at.is_empty());
        // `model` is null; the effective model lives in the snapshot.
        assert_eq!(r.model, "gpt-5.5");
    }

    #[test]
    fn explicit_model_override_wins_over_the_snapshot() {
        let mut j = job();
        j["model_snapshot"] = json!("gpt-5.5");
        j["model"] = json!("anthropic/claude");
        assert_eq!(routine_from_job(&j).model, "anthropic/claude");
    }

    #[test]
    fn derive_name_fills_in_what_the_gateway_demands() {
        assert_eq!(derive_name("Morning brief", "anything"), "Morning brief");
        assert_eq!(
            derive_name("  ", "Summarize what's due today"),
            "Summarize what's due today"
        );
        // Long prompts are truncated, like the gateway's own default.
        let long = "x".repeat(120);
        assert_eq!(derive_name("", &long).chars().count(), 50);
        // Nothing to derive from at all still yields a valid name.
        assert_eq!(derive_name("", "   "), "routine");
    }

    #[test]
    fn missing_fields_degrade_instead_of_failing() {
        let r = routine_from_job(&json!({ "id": "x" }));
        assert_eq!(r.id, "x");
        assert!(r.name.is_empty());
        assert!(r.schedule.is_empty());
        // Absent `enabled` means enabled — a job the gateway lists
        // without the flag is live.
        assert!(r.enabled);
        assert_eq!(r.runs_completed, 0);
    }
}
