//! `task` — see the `task_cli` library.

fn main() {
    task_release_version::set(task_release_version::of_this_build!());
    task_cli::main();
}
