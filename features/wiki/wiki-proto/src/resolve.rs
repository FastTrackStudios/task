//! Resolving a reference against the *reader's* subscriptions.
//!
//! The rule this implements is `wiki.subscribe.resolution`, and it is
//! the one most likely to be got subtly wrong: a reference resolves
//! when its target is in the **reader's** set, whoever wrote the
//! reference and wherever the page carrying it lives.
//!
//! So resolution takes the reader's subscriptions and nothing about
//! the author. Subscribe to Music Theory and to Audio Production, and
//! Audio Production's references into Music Theory resolve for you —
//! not because Audio Production subscribes to it, but because you do.
//! Two readers of the same page legitimately see different references
//! resolve, and neither is an error.
//!
//! What non-transitivity withholds is acquisition, never rendering
//! (`wiki.subscribe.transitive`): nothing here ever consults what a
//! subscribed source itself subscribes to.

use crate::reference::Reference;
use crate::subscription::{Subscription, Unresolved};

/// A reference that found its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved<'a> {
    /// The subscription that answered.
    pub via: &'a Subscription,
    /// The page or address within it.
    pub target: &'a str,
    /// The block anchor, if the reference named one.
    pub anchor: Option<&'a str>,
}

/// Resolve one reference against a reader's subscriptions.
///
/// Local references (`[[Page]]`) return `Ok(None)`: they are the
/// reader's own tree and this function has no opinion about them. It
/// never reaches into a subscribed source for an unqualified name.
///
/// # Errors
///
/// [`Unresolved`], which distinguishes an unheld source from an
/// ambiguous one — a reader told "no such page" when the truth is "you
/// do not subscribe to that" will go looking in the wrong place.
///
/// t[impl wiki.subscribe.resolution] — the only input besides the
/// reference is the *reader's* subscriptions; nothing about the author
/// or the page's home is consulted. t[impl wiki.subscribe.transitive]
/// — nor is anything a subscribed source itself subscribes to: a source
/// the reader has not taken on is one they do not have.
pub fn resolve<'a>(
    reference: &'a Reference,
    subscriptions: &'a [Subscription],
) -> Result<Option<Resolved<'a>>, Unresolved> {
    let Some(slug) = reference.source.as_deref() else {
        // t[impl wiki.ref.format] — an unqualified reference is local
        // and never silently reaches into a subscribed source.
        return Ok(None);
    };

    let active = subscriptions.iter().filter(|s| s.is_active());

    let via = match reference.domain.as_deref() {
        // Qualified: exactly one thing can match, and a domain cannot
        // collide, so there is nothing to disambiguate.
        Some(domain) => active
            .filter(|s| s.domain == domain && s.slug == slug)
            .next()
            .ok_or_else(|| Unresolved::NoSubscription(format!("{domain}/{slug}")))?,
        // Short form: matched by slug across everything held, and
        // ambiguity is reported with its candidates rather than
        // resolved by order — picking one would make the same text
        // mean different things on different days.
        None => {
            let matches: Vec<&'a Subscription> = active.filter(|s| s.slug == slug).collect();
            let mut held = matches.into_iter();
            let Some(first) = held.next() else {
                return Err(Unresolved::NoSubscription(slug.to_owned()));
            };
            let rest: Vec<&'a Subscription> = held.collect();
            if rest.is_empty() {
                first
            } else {
                let mut candidates = vec![first.qualified()];
                candidates.extend(rest.iter().map(|s| s.qualified()));
                return Err(Unresolved::Ambiguous {
                    slug: slug.to_owned(),
                    candidates,
                });
            }
        }
    };

    Ok(Some(Resolved {
        via,
        target: &reference.target,
        anchor: reference.anchor.as_deref(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::SourceKind;

    fn sub(domain: &str, slug: &str) -> Subscription {
        Subscription {
            domain: domain.into(),
            slug: slug.into(),
            kind: SourceKind::Wiki,
            title: slug.into(),
            core: false,
            declined: false,
        }
    }

    /// t[verify wiki.subscribe.resolution] — the worked example from
    /// the spec: Audio Production's reference into Music Theory
    /// resolves for a reader who holds Music Theory, and the reason it
    /// resolves is the reader's own subscription.
    #[test]
    fn a_reference_resolves_on_the_readers_own_set() {
        let reader = [sub("acme.test", "music-theory")];
        let written_by_someone_else = Reference::parse("acme.test/music-theory::Ionian@2026-09-01");
        let hit = resolve(&written_by_someone_else, &reader)
            .expect("resolves")
            .expect("not local");
        assert_eq!(hit.via.slug, "music-theory");
        assert_eq!(hit.target, "Ionian");
    }

    /// t[verify wiki.subscribe.transitive] — the same reference, for a
    /// reader who does not hold Music Theory, is unresolved *and says
    /// which source it wanted*. Nothing consults what the page's own
    /// wiki subscribes to.
    #[test]
    fn a_reader_without_the_subscription_is_told_which_source() {
        let reader = [sub("acme.test", "audio-production")];
        let r = Reference::parse("acme.test/music-theory::Ionian");
        let err = resolve(&r, &reader).expect_err("must not resolve");
        assert_eq!(
            err,
            Unresolved::NoSubscription("acme.test/music-theory".into())
        );
        // The message offers the thing to subscribe to, not a missing
        // page.
        assert!(err.to_string().contains("acme.test/music-theory"));
    }

    #[test]
    fn two_readers_of_one_page_legitimately_differ() {
        let page = Reference::parse("acme.test/music-theory::Modes");
        let subscribed = [sub("acme.test", "music-theory")];
        let not = [sub("alice.test", "cooking")];
        assert!(resolve(&page, &subscribed).is_ok());
        assert!(resolve(&page, &not).is_err());
    }

    #[test]
    fn a_local_reference_never_reaches_into_a_subscription() {
        let subs = [sub("acme.test", "music-theory")];
        // A subscribed wiki has a page called Ionian; an unqualified
        // reference must still be local.
        let r = Reference::parse("Ionian");
        assert_eq!(resolve(&r, &subs).unwrap(), None);
    }

    /// t[verify wiki.ref.format] — ambiguity is reported with the
    /// candidates, not guessed.
    #[test]
    fn a_short_reference_matching_two_sources_is_ambiguous() {
        let subs = [sub("acme.test", "theory"), sub("other.test", "theory")];
        let r = Reference::parse("theory::Ionian");
        match resolve(&r, &subs).expect_err("ambiguous") {
            Unresolved::Ambiguous { slug, candidates } => {
                assert_eq!(slug, "theory");
                assert_eq!(candidates.len(), 2);
                assert!(candidates.contains(&"acme.test/theory".to_owned()));
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn a_short_reference_with_one_match_resolves() {
        let subs = [Subscription {
            kind: SourceKind::Resource,
            ..sub("fasttrackstudio.app", "bible")
        }];
        let r = Reference::parse("bible::John.3.16");
        let hit = resolve(&r, &subs).unwrap().unwrap();
        assert_eq!(hit.target, "John.3.16");
        assert!(!hit.via.kind.is_editable());
    }

    /// t[verify wiki.core.optional] — a declined core subscription is
    /// held but inactive, so it stops resolving without being deleted.
    #[test]
    fn a_declined_subscription_stops_resolving() {
        let mut subs = [sub("acme.test", "music-theory")];
        subs[0].core = true;
        let r = Reference::parse("acme.test/music-theory::Ionian");
        assert!(resolve(&r, &subs).is_ok());
        subs[0].declined = true;
        assert!(resolve(&r, &subs).is_err());
    }
}
