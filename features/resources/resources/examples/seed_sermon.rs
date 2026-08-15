//! Seed the sermon "God Restores Broken People" (Crossroads Church,
//! 1 Peter 5) into the live vault:
//!   1. ingest the YouTube transcript → `<slug>.transcript.json` sidecar,
//!   2. create the timestamped scripture-citation + key-moment annotations
//!      (`sermon:slug#t:<secs> → verse:osis` / `→ topic:slug`) in
//!      `<org>/links.jsonl`, plus the annotation sidecar.
//!
//! Run:  cargo run -p resources --example seed_sermon -- [TRANSCRIPT_JSON] [ORG_ROOT]
//!   TRANSCRIPT_JSON defaults to /tmp/sermon_transcript.json
//!   ORG_ROOT        defaults to ~/.task/orgs/codywright

use std::path::PathBuf;

use links::Store;
use links_proto::{Confidence, LinksService, NodeRef, Relation, Visibility};
use resources::build::{AnnotationSpec, Target};
use resources::{Built, Transcript, TranscriptSegment, build, sidecar, transcript};

use Confidence::{Certain, Likely};
use Relation::{AlludesTo, Quotes, Tagged};

const SLUG: &str = "god-restores-broken-people";

/// A scripture citation he makes: `(secs, osis, relation, confidence, spoken_text)`.
fn citations() -> Vec<(u32, &'static str, Relation, Confidence, &'static str)> {
    vec![
        (
            109,
            "Matt.26.33",
            Quotes,
            Certain,
            "On the night Jesus was arrested, Peter boldly declared to Jesus' face, 'I will never leave you, even though all the rest do.'",
        ),
        (
            249,
            "Luke.22.54-Luke.22.62",
            AlludesTo,
            Certain,
            "And yet hours later, in three different instances, Peter would deny that he even knew who Jesus was — three times.",
        ),
        (
            326,
            "John.21.15-John.21.17",
            AlludesTo,
            Certain,
            "After the resurrection, Jesus didn't replace Peter, he restored him. In John 21 he asked three times, 'Do you love me?' — and each time, 'Feed my sheep.'",
        ),
        (
            508,
            "1Pet.5.1-1Pet.5.4",
            Quotes,
            Certain,
            "A word to you who are elders: care for the flock God has entrusted to you... and when the Great Shepherd appears you will receive a crown of never-ending glory.",
        ),
        (
            715,
            "1Pet.5.5-1Pet.5.6",
            Quotes,
            Certain,
            "All of you, dress yourselves in humility, for God opposes the proud but gives grace to the humble. Humble yourselves under the mighty power of God, and at the right time he will lift you up.",
        ),
        (
            1114,
            "Jas.4.6",
            Quotes,
            Certain,
            "James 4:6 says, 'God opposes the proud but gives grace to the humble.' Both Peter and James are quoting their scriptures, our Old Testament.",
        ),
        (
            1275,
            "Exod.7.13",
            Quotes,
            Certain,
            "Exodus 7:13 says, 'But Pharaoh's heart remained hard, and he refused to listen, just as the Lord had predicted.' He stood in opposition to God.",
        ),
        (
            1336,
            "1Sam.15.1-1Sam.15.23",
            AlludesTo,
            Likely,
            "Saul, the first king of Israel, resisted God's careful instructions and eventually lost his kingdom because he obeyed God only selectively.",
        ),
        (
            1412,
            "1Sam.15.22",
            Quotes,
            Certain,
            "1 Samuel 15:22: 'What is more pleasing to the Lord, your sacrifices, or your obedience to his voice? Obedience is better than sacrifice.'",
        ),
        (
            1527,
            "1Pet.5.7",
            Quotes,
            Certain,
            "1 Peter 5:7: 'So give all your worries and cares to God, for he cares about you.'",
        ),
        (
            1782,
            "Matt.11.28",
            Quotes,
            Certain,
            "Matthew 11:28: 'Come to me, all of you who are weary and carry heavy burdens, and I will give you rest.'",
        ),
        (
            2271,
            "1Pet.5.10",
            Quotes,
            Certain,
            "Verse 10: 'After you have suffered a little while, he will restore you, support you, strengthen you, and place you on a firm foundation.'",
        ),
    ]
}

/// A key moment / quotable line: `(secs, topic_slug, quote)`.
fn key_moments() -> Vec<(u32, &'static str, &'static str)> {
    vec![
        (
            348,
            "restoration",
            "Jesus restored him, letting him know that your greatest failure doesn't define who you are.",
        ),
        (
            369,
            "restoration",
            "Jesus comes not just to forgive, but also to restore.",
        ),
        (430, "humility", "God's grace covers humble people."),
        (
            1012,
            "humility",
            "Humility isn't thinking less of yourself; it's thinking of yourself less.",
        ),
        (
            1088,
            "pride",
            "Pride convinces us that we're superhuman; humility reminds us that we're human and all in need of a savior.",
        ),
        (
            1562,
            "anxiety",
            "One of the greatest signs of pride is actually anxiety, because anxiety declares that everything depends on me.",
        ),
        (
            1854,
            "surrender",
            "If it's big enough to worry about, then it's big enough to cast on Jesus.",
        ),
        (
            1867,
            "surrender",
            "What you keep carrying, God cannot carry for you — not because he lacks power, but because it requires trust.",
        ),
        (
            1942,
            "surrender",
            "Name it, pray it, surrender it, leave it.",
        ),
        (
            2386,
            "gods-grace",
            "Your worst moment does not get the final word — God's grace does.",
        ),
        (
            2453,
            "gods-grace",
            "Grace is receiving something you didn't deserve; none of us deserve it, but he freely gives it.",
        ),
    ]
}

fn specs() -> Vec<AnnotationSpec> {
    let mut out = Vec::new();
    for (secs, osis, rel, conf, said) in citations() {
        out.push(AnnotationSpec::moment(
            secs,
            said,
            vec![Target::verse(osis, rel, conf)],
        ));
    }
    for (secs, topic, quote) in key_moments() {
        out.push(AnnotationSpec::moment(
            secs,
            quote,
            vec![Target::topic(topic, Tagged, Likely)],
        ));
    }
    out
}

fn main() {
    let mut args = std::env::args().skip(1);
    let transcript_json = args
        .next()
        .unwrap_or_else(|| "/tmp/sermon_transcript.json".to_string());
    let org = args.next().map_or_else(
        || {
            let home = std::env::var("HOME").expect("HOME");
            PathBuf::from(home).join(".task/orgs/codywright")
        },
        PathBuf::from,
    );

    let resource_md = org.join(format!("resources/sermons/{SLUG}.md"));

    // 1. Ingest the transcript → sidecar.
    let raw = std::fs::read_to_string(&transcript_json).expect("read transcript json");
    let segments: Vec<TranscriptSegment> =
        serde_json::from_str(&raw).expect("parse transcript segments");
    let mut t = Transcript::new(SLUG, "youtube-auto");
    t.segments = segments;
    let dur_min = t.duration() / 60.0;
    transcript::save(transcript::transcript_path(&resource_md), &t).expect("write transcript");

    // 2. Annotations → links + sidecar.
    let Built { links, sidecar: sc } = build(
        NodeRef::sermon(SLUG),
        specs(),
        Visibility::Public,
        "claude-analysis",
        &format!("{SLUG}-analysis"),
    );
    sidecar::save(
        org.join(format!("resources/sermons/{SLUG}.annotations.json")),
        &sc,
    )
    .expect("write annotation sidecar");

    let store = Store::open(org.join("links.jsonl"));
    let existing = store
        .graph(Confidence::Speculative, true)
        .expect("read links");
    let src = format!("{SLUG}-analysis");
    for l in existing
        .into_iter()
        .filter(|l| l.provenance.source_ref == src)
    {
        store.delete(&l.id).expect("delete stale");
    }
    let n = links.len();
    for link in links {
        store.create(link).expect("create link");
    }

    println!(
        "{SLUG}: transcript {} cues ({dur_min:.1} min) → sidecar; {} annotations, {n} links → links.jsonl",
        t.segments.len(),
        sc.annotations.len()
    );
}
