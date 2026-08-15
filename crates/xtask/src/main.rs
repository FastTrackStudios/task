//! Development tasks for the Task app.
//!
//! Run with: `cargo xtask <command>`
//!
//! Commands:
//!   build             — Build WASM + JS bundle for the Obsidian plugin
//!   codegen           — Generate TypeScript bindings from Vox services

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map_or("help", std::string::String::as_str);

    match command {
        "build" => {
            let workspace_root = workspace_root();
            build_obsidian_plugin(&workspace_root)?;
        }
        "codegen" => {
            let workspace_root = workspace_root();
            codegen_typescript(&workspace_root)?;
        }
        "help" | "--help" | "-h" => {
            println!("cargo xtask <command>");
            println!();
            println!("Commands:");
            println!("  build             Build WASM + JS bundle for the Obsidian plugin");
            println!("  codegen           Generate TypeScript bindings");
        }
        other => {
            eprintln!("Unknown command: {other}");
            eprintln!("Run `cargo xtask help` for usage.");
            std::process::exit(1);
        }
    }

    Ok(())
}

fn workspace_root() -> std::path::PathBuf {
    // The xtask binary is run from the workspace root by cargo.
    // CARGO_MANIFEST_DIR points to the xtask/ crate, so go up one level.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR must be set (run via `cargo xtask`)");
    Path::new(&manifest_dir)
        .parent()
        .expect("xtask must be inside the workspace")
        .to_path_buf()
}

fn codegen_typescript(workspace_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // ui-lab is the only TS consumer today. The Obsidian plugin will
    // get its own out-dir entry here when it grows vox clients.
    let out_dir = workspace_root.join("ui-lab").join("src").join("generated");
    std::fs::create_dir_all(&out_dir)?;

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

fn build_obsidian_plugin(workspace_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use xshell::{Shell, cmd};

    let plugin_dir = workspace_root
        .join("integrations")
        .join("obsidian")
        .join("plugin");
    let sh = Shell::new()?;
    sh.change_dir(&plugin_dir);

    // Step 1: compile Rust → WASM and generate JS bindings via wasm-pack
    eprintln!("==> wasm-pack build --target web");
    cmd!(
        sh,
        "wasm-pack build --target web --out-dir pkg --out-name task_task_core"
    )
    .run()?;

    // Step 2: install npm deps if not already present
    if !plugin_dir.join("node_modules").exists() {
        eprintln!("==> npm install");
        cmd!(sh, "npm install").run()?;
    }

    // Step 3: bundle TypeScript + inlined WASM → main.js
    eprintln!("==> npm run build");
    cmd!(sh, "npm run build").run()?;

    println!("Built: {}", plugin_dir.join("main.js").display());
    Ok(())
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
