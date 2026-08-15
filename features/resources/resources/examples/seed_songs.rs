//! Seed the two worship-song scriptural analyses into the live vault:
//! for each song, write its annotation **sidecar**
//! (`resources/songs/<slug>.annotations.json`) and create the
//! `song:slug#anchor → verse:osis` **typed links** in `<org>/links.jsonl`.
//!
//! This is the headless authoring path (`resources::build` →
//! `links::Store`) — the same shape a future "annotate" button calls.
//!
//! Run:  cargo run -p resources --example seed_songs -- [ORG_ROOT]
//! ORG_ROOT defaults to ~/.task/orgs/codywright.

use std::path::{Path, PathBuf};

use links::Store;
use links_proto::{Confidence, LinksService, NodeRef, Relation, Visibility};
use resources::build::{AnnotationSpec, Target};
use resources::{Built, build, sidecar};

use Confidence::{Certain, Likely, Possible};
use Relation::{AlludesTo, Mentions, Quotes};

/// `t(osis, relation, confidence)` — terse `Target` constructor.
fn t(osis: &str, r: Relation, c: Confidence) -> Target {
    Target::verse(osis, r, c)
}

/// `s(anchor, text, targets)` — terse span-annotation constructor.
fn s(anchor: &str, text: &str, targets: Vec<Target>) -> AnnotationSpec {
    AnnotationSpec::span(anchor, text, targets)
}

fn keep_on_finding_more() -> Vec<AnnotationSpec> {
    vec![
        s("verse1.L1", "More than I ask, imagine, or dream", vec![t("Eph.3.20", AlludesTo, Certain)]),
        s("verse1.L2", "You're better, You're better", vec![t("Ps.84.10", AlludesTo, Likely), t("Ps.63.3", AlludesTo, Likely)]),
        s("verse1.L3", "I think of the things too good to believe", vec![t("1Cor.2.9", AlludesTo, Likely), t("Luke.24.41", AlludesTo, Possible)]),
        s("chorus.L1", "You are the maker", vec![t("Gen.1.1", Mentions, Certain), t("John.1.3", AlludesTo, Certain), t("Col.1.16", AlludesTo, Certain), t("Isa.40.28", AlludesTo, Likely), t("Ps.95.6", AlludesTo, Likely)]),
        s("chorus.L2", "You're the meaning of life", vec![t("Col.1.17", AlludesTo, Likely), t("Acts.17.28", AlludesTo, Likely), t("Eccl.12.13", AlludesTo, Possible)]),
        s("chorus.L3", "You're the heart that is beating inside", vec![t("Ezek.36.26", AlludesTo, Likely), t("Gal.2.20", AlludesTo, Likely), t("Acts.17.25", AlludesTo, Possible)]),
        s("chorus.L4", "Lord, open my mind", vec![t("Luke.24.45", AlludesTo, Certain), t("Ps.119.18", AlludesTo, Likely)]),
        s("chorus.L5", "The more that I seek You, the more that I find", vec![t("Jer.29.13", AlludesTo, Certain), t("Matt.7.7-Matt.7.8", AlludesTo, Certain), t("Prov.8.17", AlludesTo, Likely), t("Deut.4.29", AlludesTo, Likely)]),
        s("verse2.L1", "Brighter than stars, You speak to the dark", vec![t("Rev.22.16", AlludesTo, Likely), t("Rev.21.23", AlludesTo, Likely), t("Dan.12.3", AlludesTo, Possible), t("2Pet.1.19", AlludesTo, Possible), t("Gen.1.3", AlludesTo, Certain), t("2Cor.4.6", AlludesTo, Certain), t("John.1.5", AlludesTo, Likely)]),
        s("verse2.L2", "Surrender, surrender", vec![t("Rom.12.1", AlludesTo, Likely), t("Matt.16.24", AlludesTo, Possible), t("Gal.2.20", AlludesTo, Possible)]),
        s("verse2.L3", "Let the song of my heart be Glory to God", vec![t("Luke.2.14", Quotes, Certain), t("Ps.40.3", AlludesTo, Likely), t("1Cor.10.31", AlludesTo, Likely), t("Ps.19.1", AlludesTo, Possible)]),
        s("verse2.L4", "Forever, forever", vec![t("Ps.136.1", AlludesTo, Likely), t("Ps.23.6", AlludesTo, Likely), t("Rev.4.9", AlludesTo, Possible)]),
        s("bridge.L1", "You're a friend and a father", vec![t("John.15.15", Mentions, Certain), t("Jas.2.23", AlludesTo, Likely), t("Exod.33.11", AlludesTo, Likely), t("Rom.8.15", Mentions, Certain), t("Matt.6.9", AlludesTo, Likely), t("2Cor.6.18", AlludesTo, Likely)]),
        s("bridge.L2", "You're the story, the author", vec![t("Heb.12.2", AlludesTo, Certain), t("Rev.1.8", AlludesTo, Likely), t("Rev.22.13", AlludesTo, Likely), t("Acts.3.15", AlludesTo, Possible)]),
        s("bridge.L3", "You're the well and the water", vec![t("John.4.14", AlludesTo, Certain), t("Jer.2.13", AlludesTo, Likely), t("John.7.38", AlludesTo, Likely), t("Rev.21.6", AlludesTo, Likely), t("Ps.36.9", AlludesTo, Likely)]),
        s("bridge.L4", "The more I seek, I keep on finding more", vec![t("Jer.29.13", AlludesTo, Certain), t("Matt.7.7-Matt.7.8", AlludesTo, Certain), t("Ps.27.4", AlludesTo, Possible), t("Phil.3.12", AlludesTo, Possible)]),
        s("bridge.L5", "If I'm living or dying", vec![t("Rom.14.8", AlludesTo, Certain), t("Phil.1.21", AlludesTo, Likely), t("Rom.8.38-Rom.8.39", AlludesTo, Likely)]),
        s("bridge.L6", "You're the one I delight in", vec![t("Ps.37.4", AlludesTo, Certain), t("Ps.1.2", AlludesTo, Likely), t("Isa.58.14", AlludesTo, Likely), t("Ps.73.25", AlludesTo, Likely)]),
        s("bridge.L7", "From Eden to Zion", vec![t("Gen.2.8-Gen.2.10", AlludesTo, Certain), t("Heb.12.22", AlludesTo, Certain), t("Rev.21.2", AlludesTo, Likely), t("Rev.22.1-Rev.22.2", AlludesTo, Certain), t("Ps.132.13", AlludesTo, Likely)])
            .with_note("The river of Eden (Gen 2:10) becomes the river of life (Rev 22:1) — 'the well and the water' and 'from Eden to Zion' are the same motif bracketing Scripture."),
        s("tag.L1", "The more I seek, I keep on finding more", vec![t("Jer.29.13", AlludesTo, Certain), t("Matt.7.7-Matt.7.8", AlludesTo, Certain)]),
    ]
}

fn a_forgiving_god() -> Vec<AnnotationSpec> {
    let parable = "Retells the parable of the Prodigal Son (Luke 15:11-32).";
    vec![
        s(
            "v1.L1",
            "I left my Father when I was young",
            vec![
                t("Luke.15.12", AlludesTo, Certain),
                t("Luke.15.13", AlludesTo, Certain),
            ],
        )
        .with_note(parable),
        s(
            "v1.L2",
            "Wanted the world so I spent it all",
            vec![
                t("Luke.15.13", AlludesTo, Certain),
                t("1John.2.16", AlludesTo, Likely),
            ],
        ),
        s(
            "v1.L3",
            "Searching for something I didn't know",
            vec![
                t("Luke.15.14-Luke.15.16", AlludesTo, Likely),
                t("Eccl.1.14", AlludesTo, Possible),
            ],
        ),
        s(
            "v1.L4",
            "Was under the roof of my Father's home",
            vec![t("Luke.15.17", AlludesTo, Likely)],
        ),
        s(
            "c1.L1",
            "But He's never broke a promise",
            vec![
                t("Num.23.19", AlludesTo, Likely),
                t("Josh.21.45", AlludesTo, Likely),
                t("Heb.10.23", AlludesTo, Likely),
            ],
        ),
        s(
            "c1.L2",
            "Listen cuz He's calling",
            vec![
                t("Rev.3.20", AlludesTo, Possible),
                t("Isa.55.1", AlludesTo, Possible),
            ],
        ),
        s(
            "c1.L3",
            "Jesus says He wants ya",
            vec![
                t("Matt.11.28", AlludesTo, Likely),
                t("1Tim.2.4", AlludesTo, Possible),
            ],
        ),
        s(
            "c1.L4",
            "He's a forgiving God",
            vec![
                t("Neh.9.17", AlludesTo, Certain),
                t("Ps.86.5", AlludesTo, Certain),
                t("Dan.9.9", AlludesTo, Likely),
                t("Mic.7.18", AlludesTo, Likely),
                t("1John.1.9", AlludesTo, Likely),
            ],
        ),
        s(
            "i1.L1",
            "He's a forgiving God (interlude)",
            vec![
                t("Neh.9.17", AlludesTo, Certain),
                t("Mic.7.18", AlludesTo, Likely),
            ],
        ),
        s(
            "v2.L1",
            "I've sinned against Him more ways than one",
            vec![
                t("Luke.15.18", AlludesTo, Certain),
                t("Rom.3.23", AlludesTo, Likely),
            ],
        ),
        s(
            "v2.L2",
            "Didn't deserve to be called His son",
            vec![
                t("Luke.15.19", AlludesTo, Certain),
                t("Luke.15.21", AlludesTo, Certain),
            ],
        ),
        s(
            "v2.L3",
            "No greater love have I ever known",
            vec![
                t("John.15.13", AlludesTo, Certain),
                t("Rom.5.8", AlludesTo, Likely),
            ],
        ),
        s(
            "v2.L4",
            "My Father ran to me, said welcome home",
            vec![t("Luke.15.20", AlludesTo, Certain)],
        )
        .with_note("An ancient father runs — the scandal of grace."),
        s(
            "c2.L5",
            "No longer have to wander",
            vec![t("Luke.15.24", AlludesTo, Likely)],
        ),
        s(
            "c2.L6",
            "Calling sons and daughters",
            vec![t("2Cor.6.18", AlludesTo, Certain)],
        ),
        s(
            "c2.L7",
            "After all He is the Father",
            vec![
                t("1John.3.1", AlludesTo, Likely),
                t("Luke.15.20", AlludesTo, Likely),
            ],
        ),
        s(
            "b1.L1",
            "He's been sitting on the porch, awaiting your arrival",
            vec![t("Luke.15.20", AlludesTo, Certain)],
        ),
        s(
            "b1.L2",
            "You never left His heart, He's always thinking of you",
            vec![
                t("Jer.31.20", AlludesTo, Likely),
                t("Isa.49.15-Isa.49.16", AlludesTo, Likely),
            ],
        ),
        s(
            "b1.L3",
            "He'll come running out to meet you, and throw His arms around you",
            vec![t("Luke.15.20", AlludesTo, Certain)],
        ),
        s(
            "b1.L4",
            "He's a forgiving God",
            vec![
                t("Ps.103.12", AlludesTo, Likely),
                t("Mic.7.19", AlludesTo, Likely),
            ],
        ),
        s(
            "b2.L1",
            "You've been out on your own, now it's time to come home",
            vec![t("Luke.15.18", AlludesTo, Certain)],
        ),
        s(
            "b2.L2",
            "Just run to Jesus",
            vec![
                t("Heb.4.16", AlludesTo, Likely),
                t("Matt.11.28", AlludesTo, Likely),
            ],
        ),
    ]
}

fn seed_song(org: &Path, store: &Store, slug: &str, specs: Vec<AnnotationSpec>) {
    let source_ref = format!("{slug}-analysis");
    let Built { links, sidecar: sc } = build(
        NodeRef::song(slug),
        specs,
        Visibility::Public,
        "claude-analysis",
        &source_ref,
    );

    // Sidecar next to the resource file.
    let sc_path = org.join(format!("resources/songs/{slug}.annotations.json"));
    sidecar::save(&sc_path, &sc).expect("write sidecar");

    // Idempotent: drop any links a prior run of this analysis left behind.
    let existing = store
        .graph(Confidence::Speculative, true)
        .expect("read links");
    let mut removed = 0usize;
    for l in existing {
        if l.provenance.source_ref == source_ref {
            store.delete(&l.id).expect("delete stale link");
            removed += 1;
        }
    }

    // Links into <org>/links.jsonl.
    let mut created = 0usize;
    for link in links {
        store.create(link).expect("create link");
        created += 1;
    }
    if removed > 0 {
        println!("  (replaced {removed} links from a prior run)");
    }
    println!(
        "{slug}: {created} links → links.jsonl, {} annotations → {}",
        sc.annotations.len(),
        sc_path.display()
    );
}

fn main() {
    let org: PathBuf = std::env::args().nth(1).map_or_else(
        || {
            let home = std::env::var("HOME").expect("HOME");
            PathBuf::from(home).join(".task/orgs/codywright")
        },
        PathBuf::from,
    );

    let store = Store::open(org.join("links.jsonl"));
    seed_song(&org, &store, "keep-on-finding-more", keep_on_finding_more());
    seed_song(&org, &store, "a-forgiving-god", a_forgiving_god());
}
