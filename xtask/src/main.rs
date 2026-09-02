//! Development tasks for the Task app.
//!
//! Run with: `cargo xtask <command>`
//!
//! Commands:
//!   codegen `<out-dir>` — Generate TypeScript bindings from Vox services

use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map_or("help", std::string::String::as_str);

    match command {
        "codegen" => {
            let Some(out_dir) = args.get(1) else {
                eprintln!("codegen needs an output directory:");
                eprintln!("  cargo xtask codegen <out-dir>");
                std::process::exit(1);
            };
            codegen_typescript(&PathBuf::from(out_dir))?;
        }
        "help" | "--help" | "-h" => {
            println!("cargo xtask <command>");
            println!();
            println!("Commands:");
            println!("  codegen <out-dir>   Generate TypeScript bindings into <out-dir>");
        }
        other => {
            eprintln!("Unknown command: {other}");
            eprintln!("Run `cargo xtask help` for usage.");
            std::process::exit(1);
        }
    }

    Ok(())
}

// The out-dir is a parameter rather than a hardcoded path: `ui-lab` was
// the only in-tree TS consumer and it has been removed, so there is no
// longer a default worth guessing. Pass the consumer's generated/ dir.
fn codegen_typescript(out_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(out_dir)?;

    for service in service_descriptors() {
        let ts = vox_codegen::targets::typescript::generate_service(service);
        let filename = format!(
            "{}.generated.ts",
            service.service_name.to_lowercase().replace(' ', "-")
        );
        let out_path = out_dir.join(&filename);
        write_if_changed(&out_path, ts)?;
        println!("Generated TypeScript: {}", out_path.display());
    }
    Ok(())
}

fn service_descriptors() -> Vec<&'static vox_types::ServiceDescriptor> {
    // Per-feature service descriptors, pulled from the features/*
    // proto crates (each #[architect::rpc] trait emits a
    // `<snake_name>_service_descriptor()` under the `vox` feature).
    // Add new services here as their TS clients are needed.
    //
    // Traits with `#[subscribe]` declarations also emit a stream
    // sibling (`<Trait>Stream` — a vox service whose methods take a
    // `Tx<Event>` sink). Those descriptors are listed too, so the
    // generated TS exposes the subscription streams: create a
    // channel pair (`channel<TaskEvent>()` from @bearcove/vox-core),
    // pass the tx to `events(tx)`, and `for await` the rx.
    vec![
        project::project_service_descriptor(),
        task::task_service_descriptor(),
        task::task_stream_descriptor(),
        auth_proto::auth_service_service_descriptor(),
        milestone_proto::milestone_service_descriptor(),
        workstream_proto::workstream_service_descriptor(),
        workstream_proto::workstream_stream_descriptor(),
    ]
}

fn write_if_changed(path: &Path, content: String) -> std::io::Result<()> {
    if path.exists() {
        let existing = std::fs::read_to_string(path)?;
        if existing == content {
            return Ok(());
        }
    }
    std::fs::write(path, content)
}
