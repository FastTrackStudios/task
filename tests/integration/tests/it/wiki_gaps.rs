//! Chapter — a wiki says where it rests on one voice.
//!
//! A study wiki built from one video, or from several by the same
//! person, looks well sourced and is not. `Graph::gaps` flags every page
//! whose `sources:` all come from one author (read from the source
//! pages' `author:`), so the wiki home can list what needs a second
//! voice and a page can say so in its strip. This drives it over the
//! wire, as an Editor writing pages.

use integration::client::Session;
use integration::scenario::Scenario;
use wiki_proto::graph::GapKind;

const WIKI: &str = "music-theory";

/// t[verify wiki.gaps.one-voice] — two sources by one author make a page
/// that cites both one voice; citing a source by someone else as well
/// makes it not.
#[tokio::test(flavor = "multi_thread")]
async fn a_page_citing_one_author_is_flagged_as_one_voice() {
    let s = Scenario::open().await;
    let alice = Session::open(&s.orgs.acme, s.people.alice.token.clone()).await;
    let pages = alice.wiki_pages().await;
    let dir = format!("Voices-{}", uuid::Uuid::new_v4().simple());
    let write = |path: String, md: String| {
        let pages = pages.clone();
        async move {
            pages
                .write_page(WIKI.to_string(), path.clone(), md, String::new())
                .await
                .unwrap_or_else(|e| panic!("write {path}: {e:?}"));
        }
    };
    for (stem, author) in [
        ("talk-a", "Ada Lovelace"),
        ("talk-b", "Ada Lovelace"),
        ("book-c", "Mark Smith"),
    ] {
        write(
            format!("{dir}/Sources/{dir}-{stem}.md"),
            format!("---\ntitle: {stem}\ntype: source\nauthor: {author}\n---\n\n# {stem}\n"),
        )
        .await;
    }
    let one = format!("{dir}/One.md");
    let two = format!("{dir}/Two.md");
    write(
        one.clone(),
        format!(
            "---\ntitle: One\ntype: topic\nsources: [\"raw/sources/{dir}-talk-a.md\", \"raw/sources/{dir}-talk-b.md\"]\n---\n\n# One\n"
        ),
    )
    .await;
    write(
        two.clone(),
        format!(
            "---\ntitle: Two\ntype: topic\nsources: [\"raw/sources/{dir}-talk-a.md\", \"raw/sources/{dir}-book-c.md\"]\n---\n\n# Two\n"
        ),
    )
    .await;

    let gaps = alice
        .wiki_graph()
        .await
        .gaps(WIKI.to_string())
        .await
        .expect("gaps");
    let one_voice = |path: &str| {
        gaps.iter()
            .find(|g| matches!(g.kind, GapKind::OneVoice) && g.subjects.iter().any(|p| p == path))
    };
    let flagged = one_voice(&one).expect("a page citing one author is one voice");
    assert!(
        flagged.explanation.contains("all by Ada Lovelace"),
        "{}",
        flagged.explanation
    );
    assert!(
        one_voice(&two).is_none(),
        "a second author is a second voice"
    );

    // t[verify wiki.gaps.style] — neither page has a summary, which the
    // style check says, page by page.
    let style = gaps
        .iter()
        .find(|g| matches!(g.kind, GapKind::Style) && g.subjects.contains(&one))
        .expect("a page without a summary has style notes");
    assert!(
        style.explanation.contains("no summary"),
        "{}",
        style.explanation
    );
}
