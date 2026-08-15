//! Generate biblical entities as **wiki pages** from BibleData CSVs.
//!
//! Reuses the wiki's `type: entity` page format — one markdown page per
//! person/place in `<dest>` (e.g. `<org>/wiki/Knowledge/Entities/`), body
//! linking the verses (`[[Genesis 1:1]]`) so the existing wiki + link +
//! backlink machinery handles them. No parallel entity store.
//!
//! ```text
//! cargo run -p scripture --example generate_entity_pages -- \
//!   <dest_dir> <Person.csv> <PersonVerse.csv> <Place.csv> <PlaceVerse.csv>
//! ```

use std::collections::HashSet;
use std::path::Path;

use scripture::{Entity, VerseId, from_bible_data};

/// How many verse links to list in a page body before summarizing.
const MAX_REFS: usize = 40;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, dest, persons, person_verse, places, place_verse] = args.as_slice() else {
        eprintln!(
            "usage: generate_entity_pages <dest> <Person.csv> <PersonVerse.csv> <Place.csv> <PlaceVerse.csv>"
        );
        std::process::exit(2);
    };
    let read = |p: &str| std::fs::read_to_string(p).expect("read csv");
    let entities = from_bible_data(
        &read(persons),
        &read(person_verse),
        &read(places),
        &read(place_verse),
    );

    let dir = Path::new(dest);
    std::fs::create_dir_all(dir).expect("mkdir");
    let mut used = HashSet::new();
    let mut written = 0;
    for e in &entities {
        if e.refs.is_empty() {
            continue; // only entities actually anchored to verses
        }
        let stem = unique_stem(&e.name, &mut used);
        std::fs::write(dir.join(format!("{stem}.md")), page(e)).expect("write page");
        written += 1;
    }
    println!("wrote {written} entity pages -> {}", dir.display());
}

/// A filesystem-safe, collision-free page stem from a display name.
fn unique_stem(name: &str, used: &mut HashSet<String>) -> String {
    let base: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let base = base.trim().to_string();
    let base = if base.is_empty() {
        "Unnamed".into()
    } else {
        base
    };
    if used.insert(base.clone()) {
        return base;
    }
    for n in 2.. {
        let candidate = format!("{base} ({n})");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

/// Render one entity as a `type: entity` wiki page.
fn page(e: &Entity) -> String {
    let mut tags = vec![e.kind.clone()];
    if !e.attribute.is_empty() {
        tags.push(e.attribute.clone());
    }
    let refs: Vec<String> = e
        .refs
        .iter()
        .filter_map(|r| VerseId::parse(r).ok())
        .map(|v| format!("[[{v}]]"))
        .collect();
    let shown = refs.len().min(MAX_REFS);
    let mut mentions = refs[..shown].join(", ");
    if refs.len() > shown {
        mentions.push_str(&format!(" _(+{} more)_", refs.len() - shown));
    }

    let desc = if e.description.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", e.description)
    };
    format!(
        "---\ntitle: {name}\ntype: entity\ntags: [{tags}]\nsources: [\"bibledata\"]\nbibledata_id: {id}\nfolder: \"[[Wiki]]\"\n---\n\n# {name}\n\n{desc}Mentioned in: {mentions}\n",
        name = e.name,
        tags = tags.join(", "),
        id = e.id,
    )
}
