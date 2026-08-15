//! Rate cascade resolution.
//!
//! Walks four levels, most-specific-first, returning the
//! first non-zero rate it finds:
//!
//! 1. `ProjectMemberRate` for `(project_id, user_id)`.
//! 2. The project markdown's `defaultRateCents` field.
//! 3. `OrgMemberRate` for `(org_id, user_id)`.
//! 4. `0` — non-billable.
//!
//! The DB query side lives in [`crate::store`]; this module
//! exposes the value type and the pure picker function so
//! tests can exercise the precedence rules without spinning
//! up a SeaORM connection.

use serde::{Deserialize, Serialize};
use timer_proto::service::RateSource;

/// Outcome of one rate-cascade lookup. Returned by
/// [`crate::Store::resolve_rate`] and snapshotted into
/// `WorkSession.rate_cents` + `currency` on stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedRate {
    pub hourly_cents: i64,
    pub currency: String,
    pub source: RateSource,
}

impl ResolvedRate {
    /// Non-billable rate. Used when no level produced a
    /// non-zero rate.
    #[must_use]
    pub fn none() -> Self {
        Self {
            hourly_cents: 0,
            currency: String::new(),
            source: RateSource::None,
        }
    }
}

/// Inputs to the cascade. Each level is `Option`-typed so
/// callers can pass `None` for levels that don't apply
/// (e.g. a session with no project_id has no
/// `ProjectMemberRate` candidate).
#[derive(Debug, Clone, Default)]
pub struct CascadeInputs {
    /// `ProjectMemberRate` candidate: `(hourly_cents,
    /// project_currency)`.
    pub project_member: Option<(i64, String)>,
    /// Project markdown defaults: `(default_rate_cents,
    /// currency)`.
    pub project_default: Option<(i64, String)>,
    /// `OrgMemberRate` candidate: `(hourly_cents,
    /// org_currency)`.
    pub org_member: Option<(i64, String)>,
}

/// Pure cascade resolver. Picks the first level with a
/// non-zero `hourly_cents`. Non-zero is the discriminator
/// (not just presence) so a row of "rate = 0" at one level
/// doesn't override the next — that's how you express
/// "this member is free on this project but billable on
/// the org default."
#[must_use]
pub fn resolve(inputs: &CascadeInputs) -> ResolvedRate {
    if let Some((cents, currency)) = &inputs.project_member {
        if *cents > 0 {
            return ResolvedRate {
                hourly_cents: *cents,
                currency: currency.clone(),
                source: RateSource::ProjectMember,
            };
        }
    }
    if let Some((cents, currency)) = &inputs.project_default {
        if *cents > 0 {
            return ResolvedRate {
                hourly_cents: *cents,
                currency: currency.clone(),
                source: RateSource::ProjectDefault,
            };
        }
    }
    if let Some((cents, currency)) = &inputs.org_member {
        if *cents > 0 {
            return ResolvedRate {
                hourly_cents: *cents,
                currency: currency.clone(),
                source: RateSource::OrgMember,
            };
        }
    }
    ResolvedRate::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_returns_none() {
        let r = resolve(&CascadeInputs::default());
        assert_eq!(r, ResolvedRate::none());
    }

    #[test]
    fn project_member_wins_over_all() {
        let r = resolve(&CascadeInputs {
            project_member: Some((15000, "USD".into())),
            project_default: Some((10000, "USD".into())),
            org_member: Some((8000, "USD".into())),
        });
        assert_eq!(r.source, RateSource::ProjectMember);
        assert_eq!(r.hourly_cents, 15000);
    }

    #[test]
    fn project_default_falls_through_when_member_rate_zero() {
        let r = resolve(&CascadeInputs {
            project_member: Some((0, "USD".into())),
            project_default: Some((12000, "USD".into())),
            org_member: Some((8000, "USD".into())),
        });
        assert_eq!(r.source, RateSource::ProjectDefault);
        assert_eq!(r.hourly_cents, 12000);
    }

    #[test]
    fn org_member_is_last_resort() {
        let r = resolve(&CascadeInputs {
            project_member: None,
            project_default: None,
            org_member: Some((9500, "EUR".into())),
        });
        assert_eq!(r.source, RateSource::OrgMember);
        assert_eq!(r.currency, "EUR");
    }
}
