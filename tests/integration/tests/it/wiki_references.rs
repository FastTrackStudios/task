//! Chapter — a reference into a wiki opens the page it names.
//!
//! ADR 0002's references are what a wiki page writes to point into
//! another wiki: `[[acme.test/music-theory::Modes@2026-09-01|modal]]`.
//! The editor renders them and a click follows them, and neither can
//! work out on its own which org `acme.test` is or which file `Modes`
//! is. `Subscriptions::resolve_reference` is the one place that answers,
//! over the wire, as a signed-in person; these are the answers the page
//! depends on.

use integration::client::Session;
use integration::scenario::Scenario;

/// t[verify wiki.ref.format] — the seeded Harmonic Series page links
/// `acme.test/music-theory::Modes@2026-09-01` and `acme.test/
/// audio-production::Equalization@2026-09-01`. Both resolve to the page
/// in ACME's wiki, whatever stamp, anchor or display text rides along; a
/// short `slug::Page` is ACME's own wiki; a page named by its title
/// rather than its filename still resolves; and a reference to a page,
/// a wiki or a domain that does not exist is `None` rather than an
/// error — which is what renders it as a missing link.
#[tokio::test(flavor = "multi_thread")]
async fn references_resolve_to_the_page_they_name() {
    let s = Scenario::open().await;
    let alice = Session::open(&s.orgs.acme, s.people.alice.token.clone()).await;
    let refs = alice.wiki_subscriptions().await;
    let resolve = |reference: &'static str| {
        let refs = refs.clone();
        async move {
            refs.resolve_reference(reference.to_string())
                .await
                .unwrap_or_else(|e| panic!("resolve {reference}: {e:?}"))
        }
    };

    let modes = resolve("[[acme.test/music-theory::Modes@2026-09-01|modal]]")
        .await
        .expect("Modes resolves");
    assert_eq!(modes.org, s.orgs.acme.slug);
    assert_eq!(modes.wiki, "music-theory");
    assert_eq!(modes.path, "Concepts/Modes.md");
    assert_eq!(modes.title, "Modes");

    let eq = resolve("acme.test/audio-production::Equalization@2026-09-01")
        .await
        .expect("Equalization resolves");
    assert_eq!(
        (eq.wiki.as_str(), eq.path.as_str()),
        ("audio-production", "Concepts/Equalization.md")
    );

    let partials = resolve("acme.test/music-theory::Harmonic Series@2026-09-01#^partials")
        .await
        .expect("an anchored reference resolves to its page");
    assert_eq!(partials.path, "Concepts/Harmonic Series.md");

    let short = resolve("music-theory::Ionian")
        .await
        .expect("a short reference is this org's own wiki");
    assert_eq!(short.path, "Concepts/Ionian.md");

    for nowhere in [
        "acme.test/music-theory::No Such Page",
        "acme.test/no-such-wiki::Modes",
        "nowhere.test/music-theory::Modes",
        "Modes",
    ] {
        assert_eq!(resolve(nowhere).await, None, "{nowhere} resolved");
    }
}
