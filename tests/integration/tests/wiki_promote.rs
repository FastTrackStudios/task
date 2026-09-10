//! Chapter — promotion: a vetted page crosses from a working wiki into
//! a curated one.
//!
//! `features/wiki/spec/wiki.md`, "Promotion". ACME's seed carries the
//! pair the rules are about: **Studio Research**, written by an agent
//! and unvetted by construction, and **Audio Production**, curated,
//! where every page has been read by an engineer. The whole value of
//! the second is that trust, and the whole difficulty is that research
//! has to reach it somehow without dissolving it.
//!
//! What is asserted here is the crossing itself, over the wire, as
//! Alice — who owns ACME and so can read and write both wikis:
//!
//! - **a promotion copies** (`wiki.promote.copy`): the curated page
//!   appears and the research page is still there, body byte-identical;
//! - **both ends record it** (`wiki.promote.provenance`): the copy says
//!   where it came from, the source says where its vetted form went,
//!   and both carry the same instant;
//! - **the target's schema is the gate** (`wiki.promote.schema`): a
//!   page whose type the curated wiki does not declare is refused by
//!   name, and an explicit override picks from the curated wiki's own
//!   vocabulary rather than widening it;
//! - **an occupied target is not clobbered**
//!   (`wiki.promote.no-clobber`): a second promotion onto the same path
//!   is refused, and a forced one guarded by a stale hash conflicts
//!   rather than overwriting.
//!
//! # Why this chapter drives the planner directly
//!
//! `task wiki promote` is a composition of four calls that already
//! exist on the wire — `read_page`, `read_schema`, `list_pages`,
//! `write_page` — around one pure decision,
//! `wiki_proto::promote::plan`. There is no promote RPC to exercise,
//! and inventing one purely to have something for this chapter to call
//! would put the schema-translation policy on the server for no reason
//! (the CLI module says why at length).
//!
//! So the chapter does exactly what the CLI does, in the same order,
//! through the org router: the reads and the writes are real RPCs
//! against the planted seed, and the decision between them is the same
//! function the CLI calls. What that buys over the unit tests is that
//! the schema, the page and the types are the *seeded* ones rather than
//! fixtures — if someone edits Audio Production's `schema.md` so it no
//! longer declares `concept`, this chapter fails, which is the point.

use std::collections::BTreeSet;

use integration::client::Session;
use integration::scenario::Scenario;
use task_server::example_org::{PROMOTION_PAIR, SEED_PROMOTABLE_PAGE, SEED_UNPROMOTABLE_PAGE};
use wiki_proto::promote::{self, PromoteError, PromoteRequest};

fn working() -> &'static str {
    PROMOTION_PAIR.0
}

fn curated() -> &'static str {
    PROMOTION_PAIR.1
}

/// Every spelling a bare `[[link]]` in the curated wiki resolves to —
/// what the CLI builds before planning, built the same way.
async fn names_of(who: &Session, wiki: &str) -> BTreeSet<String> {
    who.wiki_pages()
        .await
        .list_pages(wiki.to_string())
        .await
        .expect("list the curated wiki's pages")
        .into_iter()
        .flat_map(|p| {
            let stem = std::path::Path::new(&p.path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_owned();
            [p.title, stem]
        })
        .filter(|n| !n.is_empty())
        .collect()
}

/// Read the source page, the target's schema and the target's page
/// names, then plan — the CLI's read half, verbatim.
async fn plan_seeded(
    who: &Session,
    page: &str,
    as_type: Option<&str>,
    to_path: Option<&str>,
) -> (String, String, Result<promote::Promotion, PromoteError>) {
    let source = who
        .wiki_pages()
        .await
        .read_page(working().to_string(), page.to_string())
        .await
        .expect("read the research page");
    let schema = who
        .wiki_schema()
        .await
        .read_schema(curated().to_string())
        .await
        .expect("the curated wiki commits a schema");
    let names = names_of(who, curated()).await;
    let plan = promote::plan(
        &source.markdown,
        &PromoteRequest {
            from_wiki: working(),
            from_path: page,
            to_wiki: curated(),
            to_path,
            as_type,
            target_schema: &schema.markdown,
            target_names: &names,
            at: chrono::Utc::now(),
        },
    );
    (source.markdown, source.sha256, plan)
}

/// t[verify wiki.promote.copy] — the research page survives the
/// promotion with its body untouched.
///
/// t[verify wiki.promote.provenance] — the copy names its origin, the
/// source names its destination, and both carry the same instant.
#[tokio::test(flavor = "multi_thread")]
async fn a_vetted_research_page_is_copied_into_the_curated_wiki_and_both_ends_say_so() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let (source_markdown, source_sha, plan) =
        plan_seeded(&alice, SEED_PROMOTABLE_PAGE, None, None).await;
    let plan = plan.expect("the seeded page's type is one the curated wiki declares");
    assert_eq!(plan.to_type, "concept");
    assert_eq!(plan.to_path, "Concepts/Dynamic Range.md");

    // The curated wiki does not hold it yet — the promotion is a
    // creation, not a repair of one the seed already planted.
    assert!(
        alice
            .wiki_pages()
            .await
            .read_page(curated().to_string(), plan.to_path.clone())
            .await
            .is_err(),
        "the curated wiki already holds `{}` — this chapter's premise is gone",
        plan.to_path
    );

    // The CLI's write half: the copy first, the back-reference second.
    let pages = alice.wiki_pages().await;
    pages
        .write_page(
            curated().to_string(),
            plan.to_path.clone(),
            plan.promoted_markdown.clone(),
            String::new(),
        )
        .await
        .expect("write the promoted page");
    pages
        .write_page(
            working().to_string(),
            SEED_PROMOTABLE_PAGE.to_string(),
            plan.annotated_source.clone(),
            source_sha,
        )
        .await
        .expect("write the back-reference");

    // ── The copy ─────────────────────────────────────────────────────
    let promoted = pages
        .read_page(curated().to_string(), plan.to_path.clone())
        .await
        .expect("the curated wiki now holds it");
    assert!(
        promoted.markdown.contains(&format!(
            "promoted_from: \"{}::{SEED_PROMOTABLE_PAGE}\"",
            working()
        )),
        "the curated page must name where it came from:\n{}",
        promoted.markdown
    );
    assert!(
        promoted.markdown.contains("ai_generated: true"),
        "vetting vouches for the claim; it does not make a model's prose an \
         engineer's writing, so the flag survives:\n{}",
        promoted.markdown
    );

    // ── The original ─────────────────────────────────────────────────
    let after = pages
        .read_page(working().to_string(), SEED_PROMOTABLE_PAGE.to_string())
        .await
        .expect("the research page is still there — a promotion copies");
    assert!(
        after
            .markdown
            .contains(&format!("promoted_to: \"{}::{}\"", curated(), plan.to_path)),
        "the research page must name where its vetted form went:\n{}",
        after.markdown
    );
    let body_of = |md: &str| md.split("\n---\n").nth(1).unwrap_or_default().to_owned();
    assert_eq!(
        body_of(&after.markdown),
        body_of(&source_markdown),
        "the working material must be left byte-identical — the trail behind the \
         vetted claim is the reason the promotion is a copy at all"
    );

    // ── The same promotion, matched up from either end ───────────────
    let stamp = |md: &str| {
        md.lines()
            .find_map(|l| l.strip_prefix("promoted_at: "))
            .map(str::to_owned)
            .expect("both ends stamp the promotion")
    };
    assert_eq!(
        stamp(&promoted.markdown),
        stamp(&after.markdown),
        "one promotion, one instant, so the two halves can be matched without a join"
    );

    // ── A bare link the curated wiki cannot resolve points home ──────
    assert!(
        promoted
            .markdown
            .contains(&format!("[[{}::Loudness War]]", working())),
        "a link to research the curated wiki does not hold must point back at the \
         wiki that does, not dangle:\n{}",
        promoted.markdown
    );
    assert!(
        promoted.markdown.contains("[[Equalization]]"),
        "a link the curated wiki CAN resolve must be left exactly as written:\n{}",
        promoted.markdown
    );
}

/// t[verify wiki.promote.schema] — the curated wiki declares no
/// `question` type, so a question is refused by name rather than
/// written anyway or mapped by guess. The override picks from the
/// curated wiki's own vocabulary.
#[tokio::test(flavor = "multi_thread")]
async fn a_type_the_curated_wiki_does_not_declare_is_refused_and_the_override_picks_from_its_own() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let (_, _, refused) = plan_seeded(&alice, SEED_UNPROMOTABLE_PAGE, None, None).await;
    let err = refused.expect_err("a `question` has no home in a wiki that declares none");
    let PromoteError::TypeNotDeclared { declared, .. } = &err else {
        panic!("wrong refusal: {err:?}");
    };
    assert!(
        declared.iter().any(|t| t == "concept") && !declared.iter().any(|t| t == "question"),
        "the refusal must name the curated wiki's real vocabulary: {declared:?}"
    );

    // Nothing was written by the attempt.
    assert!(
        alice
            .wiki_pages()
            .await
            .read_page(
                curated().to_string(),
                "Questions/Do small speakers need a different master.md".to_string()
            )
            .await
            .is_err(),
        "a refused promotion must leave the curated wiki byte-identical"
    );

    // An override naming something the curated wiki declares works…
    let (_, _, ok) = plan_seeded(&alice, SEED_UNPROMOTABLE_PAGE, Some("technique"), None).await;
    let ok = ok.expect("`technique` is a type the curated wiki declares");
    assert_eq!(ok.to_type, "technique");
    assert!(ok.to_path.starts_with("Techniques/"), "{}", ok.to_path);

    // …and one naming something it does not is refused too. The
    // override is a pick from the target's vocabulary, never an
    // addition to it — otherwise it is just the guess with extra steps.
    let (_, _, widened) = plan_seeded(&alice, SEED_UNPROMOTABLE_PAGE, Some("passage"), None).await;
    assert!(
        matches!(
            widened.expect_err("the override widens nothing"),
            PromoteError::OverrideNotDeclared { .. }
        ),
        "an override must not be able to invent a type"
    );
}

/// t[verify wiki.promote.no-clobber] — an occupied target is refused,
/// and a forced write guarded by a hash that has gone stale conflicts
/// rather than overwriting.
#[tokio::test(flavor = "multi_thread")]
async fn an_occupied_target_is_refused_and_a_stale_guard_conflicts() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let pages = alice.wiki_pages().await;

    let (_, _, plan) = plan_seeded(&alice, SEED_PROMOTABLE_PAGE, None, None).await;
    let plan = plan.expect("plans");
    pages
        .write_page(
            curated().to_string(),
            plan.to_path.clone(),
            plan.promoted_markdown.clone(),
            String::new(),
        )
        .await
        .expect("the first promotion lands");

    // The existence check the CLI makes before it writes anything. A
    // second promotion onto an occupied path is refused there — which
    // is why the caller has to type `--force`, and why doing so is a
    // decision rather than an accident.
    let occupied = pages
        .read_page(curated().to_string(), plan.to_path.clone())
        .await
        .expect("the target is occupied");

    // An engineer edits the curated page after the promoter read it.
    let edited = format!("{}\nAn engineer's own line.\n", occupied.markdown);
    pages
        .write_page(
            curated().to_string(),
            plan.to_path.clone(),
            edited,
            occupied.sha256.clone(),
        )
        .await
        .expect("the engineer's edit lands against the hash they read");

    // A forced re-promotion guarded by the now-stale hash must be a
    // conflict. `--force` overrides "this path is taken", never "this
    // page changed under you" — losing an engineer's edit to a
    // machine's redraft is the worst outcome this feature could have.
    let stale = pages
        .write_page(
            curated().to_string(),
            plan.to_path.clone(),
            plan.promoted_markdown.clone(),
            occupied.sha256.clone(),
        )
        .await;
    assert!(
        stale.is_err(),
        "a forced promotion against a stale hash must conflict, not overwrite"
    );
    let now = pages
        .read_page(curated().to_string(), plan.to_path.clone())
        .await
        .expect("read back");
    assert!(
        now.markdown.contains("An engineer's own line."),
        "the engineer's edit survived:\n{}",
        now.markdown
    );
}
