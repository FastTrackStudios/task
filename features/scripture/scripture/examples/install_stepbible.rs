//! Convert STEPBible TAGNT/TAHOT source files into a normalized
//! original-language edition in the resource library.
//!
//! ```text
//! cargo run -p scripture --example install_stepbible -- \
//!   <tagnt|tahot> <id> <name> <language> <license> <dest_dir> <file...>
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use scripture::{OrigMeta, OrigText, OrigWord, VerseId, morphgnt, oshb, stepbible};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 8 {
        eprintln!(
            "usage: install_stepbible <tagnt|tahot|morphgnt|oshb> <id> <name> <language> <license> <dest> <file...>"
        );
        std::process::exit(2);
    }
    let (kind, id, name, language, license, dest) =
        (&args[1], &args[2], &args[3], &args[4], &args[5], &args[6]);

    let mut map: BTreeMap<VerseId, Vec<OrigWord>> = BTreeMap::new();
    for file in &args[7..] {
        let raw = std::fs::read_to_string(file).expect("read source");
        let rows = match kind.as_str() {
            "tahot" => stepbible::parse_tahot_rows(&raw),
            "morphgnt" => morphgnt::parse_morphgnt_rows(&raw),
            "oshb" => oshb::parse_oshb_xml(&raw),
            _ => stepbible::parse_tagnt_rows(&raw),
        };
        let n = rows.len();
        for (vid, w) in rows {
            map.entry(vid).or_default().push(w);
        }
        eprintln!("{file}: {n} words ({} verses so far)", map.len());
    }

    let text = OrigText::from_verses(map);
    let dir = Path::new(dest);
    std::fs::create_dir_all(dir).expect("mkdir");
    std::fs::write(dir.join("text.jsonl"), text.to_jsonl()).expect("write jsonl");
    let meta = OrigMeta {
        id: id.clone(),
        name: name.clone(),
        language: language.clone(),
        license: license.clone(),
    };
    std::fs::write(
        dir.join("meta.json"),
        serde_json::to_string_pretty(&meta).unwrap(),
    )
    .expect("write meta");
    println!("installed {} verses -> {}", text.len(), dir.display());
}
