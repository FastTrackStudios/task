//! `Vault::emit` end to end: a directory of notes in, generated Rust out.
//!
//! The generated file is `include!`d verbatim into a consuming crate, so
//! a quoting bug here is a compile error in someone else's repo with a
//! stack trace pointing at a line nobody wrote. These tests are the
//! guard on that: notes here carry the characters that would break a
//! naive emitter — quotes, backslashes, newlines, braces, `#`.

use std::path::PathBuf;

/// A fixture vault plus an `OUT_DIR`, cleaned up on drop.
struct Fixture {
    dir: PathBuf,
    out: PathBuf,
}

impl Fixture {
    fn new(name: &str, notes: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("ssg-build-test-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("vault");
        let out = root.join("out");
        std::fs::create_dir_all(&dir).expect("create vault dir");
        std::fs::create_dir_all(&out).expect("create out dir");
        for (slug, source) in notes {
            std::fs::write(dir.join(format!("{slug}.md")), source).expect("write note");
        }
        Self { dir, out }
    }

    /// A builder already pointed at this fixture's vault and output.
    ///
    /// Explicitly, not through `OUT_DIR`: that variable is per-process,
    /// and cargo runs these tests in parallel threads, so setting it
    /// would have every test overwrite every other test's answer.
    fn vault(&self) -> ssg_build::Vault<'_> {
        ssg_build::Vault::at(&self.dir).out_dir(&self.out)
    }

    fn generated(&self) -> String {
        std::fs::read_to_string(self.out.join("ssg_vault.rs")).expect("read generated file")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(root) = self.dir.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

#[test]
fn emits_a_static_vault_in_reading_order() {
    let fixture = Fixture::new(
        "order",
        &[
            (
                "chords",
                "---\ntitle: Chords\norder: 2\nstage: The music\n---\n\n# Chords\n\nSee [[rhythm]].\n",
            ),
            (
                "rhythm",
                "---\ntitle: Rhythm\norder: 1\nstage: The music\n---\n\n# Rhythm\n",
            ),
        ],
    );

    fixture.vault().emit();
    let out = fixture.generated();

    assert!(out.contains("pub static VAULT: ::ssg::StaticVault"));
    // Reading order, not directory order: `rhythm` is order 1.
    let rhythm = out.find(r#"slug: "rhythm""#).expect("rhythm emitted");
    let chords = out.find(r#"slug: "chords""#).expect("chords emitted");
    assert!(rhythm < chords, "pages should be emitted in reading order");

    assert!(out.contains(r#"title: "Chords""#));
    assert!(out.contains(r#"stage: "The music""#));
    assert!(out.contains(r#"links: &["rhythm"]"#));
    // The wikilink resolved to a real anchor, at build time, marked as
    // pointing back into the vault.
    assert!(out.contains(r#"<a href=\"/guide/rhythm\" data-ssg-link=\"rhythm\">rhythm</a>"#));
}

#[test]
fn note_text_that_would_break_a_naive_emitter_survives() {
    // Every character that has to be escaped to sit inside a Rust string
    // literal, plus a `{}` pair that would eat a format argument.
    let hostile = "---\ntitle: \"Quoted\"\n---\n\nA \"quote\", a \\backslash,\n\
                   a { brace } and a #hash.\n\n```rust\nlet s = \"x\\ny\";\n```\n";
    let fixture = Fixture::new("hostile", &[("hostile", hostile)]);

    fixture.vault().emit();
    let out = fixture.generated();

    assert!(out.contains(r#"title: "Quoted""#));
    // The raw source round-trips as an escaped literal, not as itself.
    assert!(out.contains(r"\\backslash"));
    assert!(
        !out.contains("\n\nA \"quote\""),
        "raw newlines leaked into the literal"
    );
}

// t[verify ssg.render.links]
#[test]
#[should_panic(expected = "broken cross-reference")]
fn a_broken_cross_reference_fails_the_build() {
    let fixture = Fixture::new("broken", &[("chords", "see [[typo]]")]);
    fixture.vault().emit();
}

#[test]
fn broken_links_can_be_downgraded_to_a_warning() {
    let fixture = Fixture::new("broken-ok", &[("chords", "see [[typo]]")]);
    fixture.vault().allow_broken_links().emit();
    assert!(fixture.generated().contains(r#"slug: "chords""#));
}

#[test]
#[should_panic(expected = "would ship an empty vault")]
fn an_empty_vault_fails_the_build() {
    let fixture = Fixture::new("empty", &[]);
    fixture.vault().emit();
}

// t[verify ssg.render.fences]
#[test]
fn a_fence_renderer_reaches_the_generated_html() {
    let fixture = Fixture::new("fence", &[("chart", "```kf\nG C Em D\n```\n")]);

    fixture
        .vault()
        .fence(|info, body| {
            (info == "kf").then(|| format!("<svg data-chart=\"{}\"></svg>", body.trim()))
        })
        .emit();

    let out = fixture.generated();
    assert!(out.contains(r#"<svg data-chart=\"G C Em D\">"#));
    assert!(!out.contains("language-kf"), "the fence should be replaced");
}

#[test]
fn the_link_base_and_static_name_are_configurable() {
    let fixture = Fixture::new("config", &[("a", "[[b]]"), ("b", "end")]);

    fixture
        .vault()
        .link_base("/docs/")
        .static_name("DOCS")
        .emit();

    let out = fixture.generated();
    assert!(out.contains("pub static DOCS: ::ssg::StaticVault"));
    assert!(out.contains(r#"href=\"/docs/b\""#));
}

// t[verify ssg.render.metadata]
#[test]
fn the_body_is_the_note_without_its_frontmatter() {
    let fixture = Fixture::new(
        "body",
        &[("chords", "---\ntitle: Chords\n---\n\n# Chords\n\nprose\n")],
    );
    fixture.vault().emit();
    let out = fixture.generated();

    // `body` carries the prose as markdown — no frontmatter, and not
    // the rendered HTML either. Hashed delimiters because the expected
    // text contains `"#`, which would close a plain `r#"…"#`.
    assert!(out.contains(r##"body: "# Chords\n\nprose\n""##));
    assert!(!out.contains(r##"body: "---"##));
}

#[test]
fn feeds_are_written_when_a_site_url_is_given() {
    let fixture = Fixture::new(
        "feeds",
        &[(
            "intro",
            "---\ntitle: Intro\nsummary: Start here\norder: 1\n---\n\n# Intro\n",
        )],
    );
    let feeds = fixture.out.join("feeds");

    fixture
        .vault()
        .feeds("https://example.test/", &feeds)
        .emit();

    let sitemap = std::fs::read_to_string(feeds.join("sitemap.xml")).expect("sitemap");
    assert!(sitemap.contains("<loc>https://example.test/guide/intro</loc>"));

    let rss = std::fs::read_to_string(feeds.join("rss.xml")).expect("rss");
    assert!(rss.contains("<title>Intro</title>"));
    assert!(rss.contains("<guid>https://example.test/guide/intro</guid>"));
}

#[test]
fn no_site_url_means_no_feeds() {
    let fixture = Fixture::new("no-feeds", &[("intro", "# Intro")]);
    fixture.vault().emit();
    assert!(!fixture.out.join("feeds").exists());
}

#[test]
fn a_page_is_undated_unless_dating_is_asked_for() {
    let fixture = Fixture::new("undated", &[("intro", "# Intro")]);
    fixture.vault().emit();
    assert!(fixture.generated().contains(r#"updated: """#));
}

#[test]
fn dating_a_vault_outside_a_git_checkout_is_not_an_error() {
    // The fixture lives in a temp directory with no repository, which
    // is also the shape a nix derivation builds in: it copies the files
    // and not the history. Every date comes back empty and the build
    // still succeeds.
    let fixture = Fixture::new("dated", &[("intro", "# Intro")]);
    fixture.vault().dates().emit();
    assert!(fixture.generated().contains(r#"updated: """#));
}
