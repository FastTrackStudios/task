//! Install a Bible translation into the resource library, then load it
//! back as a smoke test.
//!
//! ```text
//! cargo run -p scripture --example install_bible -- <src_usfm_dir> <dest_dir> <TRANSLATION>
//! ```
//!
//! `src_usfm_dir` is a directory of source USFM (e.g. an unzipped
//! eBible.org export); `dest_dir` is the translation folder in the
//! resource library (e.g. `~/.task/orgs/codywright/resources/bible/WEB`).

use std::path::Path;

use scripture::{Bible, VerseId, install_usfm_dir};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, src, dest, tx] = args.as_slice() else {
        eprintln!("usage: install_bible <src_usfm_dir> <dest_dir> <TRANSLATION>");
        std::process::exit(2);
    };

    let books = install_usfm_dir(Path::new(src), Path::new(dest)).expect("install");
    println!("installed {} books -> {dest}", books.len());

    let bible = Bible::load_dir(Path::new(dest), tx.as_str()).expect("load");
    println!("loaded {} verses for {}", bible.len(), bible.translation);

    let probe = VerseId::parse("John 3:16").expect("ref");
    match bible.get(probe) {
        Some(text) => println!("John 3:16 = {text}"),
        None => println!("John 3:16 not found (is John installed?)"),
    }
}
