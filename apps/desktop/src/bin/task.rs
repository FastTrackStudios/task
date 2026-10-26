//! The `task` CLI the app bundles (`Contents/MacOS/task`), built from this
//! package so it shares the app's build — see `task_cli`.

fn main() {
    task_release_version::set(task_release_version::of_this_build!());
    task_cli::main();
}
