//! The sync agent the app bundles (`Contents/MacOS/fts-files-daemon`),
//! built from this package so it shares the app's build — see
//! `files_daemon::cli`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    files_daemon::cli::main()
}
