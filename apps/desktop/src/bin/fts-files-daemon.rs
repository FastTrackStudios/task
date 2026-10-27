//! The sync agent the app bundles (`Contents/MacOS/fts-files-daemon`),
//! built from this package so it shares the app's build — see
//! `files_daemon::cli`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    task_release_version::set(task_release_version::of_this_build!());
    files_daemon::cli::main()
}
