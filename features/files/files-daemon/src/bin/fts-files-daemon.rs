//! `fts-files-daemon` — see [`files_daemon::cli`].

fn main() -> Result<(), Box<dyn std::error::Error>> {
    files_daemon::cli::main()
}
