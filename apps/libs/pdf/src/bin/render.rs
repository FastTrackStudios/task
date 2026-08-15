//! `task-pdf-render` — read a render request on stdin,
//! write PDF bytes on stdout.
//!
//! Run as a subprocess from any other crate that needs PDF
//! generation. Decouples the fulgur compile tree from the
//! workspace's giant interdependency graph (stylo's
//! recursion-limit issue surfaces in feature-unified
//! builds; isolating fulgur to one binary sidesteps it).
//!
//! Request shape (JSON):
//!
//! ```json
//! {
//!   "mode": "invoice",
//!   "data": { ... pdf::InvoiceData ... }
//! }
//! ```
//!
//! Or, for arbitrary HTML/CSS:
//!
//! ```json
//! { "mode": "html", "html": "<h1>…</h1>" }
//! ```
//!
//! Or, for a custom template:
//!
//! ```json
//! {
//!   "mode": "template",
//!   "name": "weekly.html",
//!   "template": "<html>…</html>",
//!   "data": { ... }
//! }
//! ```

use std::io::{Read, Write};

use clap::Parser;
use serde::Deserialize;

#[derive(Parser)]
#[command(name = "task-pdf-render", about = "Stdin JSON → stdout PDF")]
struct Cli {
    /// Optional output path. Default: stdout.
    #[arg(short, long)]
    out: Option<std::path::PathBuf>,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum Request {
    Invoice { data: pdf::InvoiceData },
    Html { html: String },
    Template {
        name: String,
        template: String,
        data: serde_json::Value,
    },
}

/// RAII guard that points fd 1 at `/dev/null` and restores
/// the original on drop. Single-threaded use only (we're in
/// `main`, before any spawn). No-op on non-unix and on any
/// dup failure — worst case is the old noisy behavior.
struct StdoutGate {
    #[cfg(unix)]
    saved: Option<i32>,
}

impl StdoutGate {
    fn divert() -> Self {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let Ok(devnull) = std::fs::OpenOptions::new().write(true).open("/dev/null") else {
                return Self { saved: None };
            };
            // SAFETY: plain dup/dup2 on process-owned fds.
            let saved = unsafe { libc::dup(1) };
            if saved < 0 || unsafe { libc::dup2(devnull.as_raw_fd(), 1) } < 0 {
                return Self { saved: None };
            }
            Self { saved: Some(saved) }
        }
        #[cfg(not(unix))]
        {
            Self {}
        }
    }
}

impl Drop for StdoutGate {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(saved) = self.saved {
            // SAFETY: restoring the fd we dup'd in `divert`.
            unsafe {
                libc::dup2(saved, 1);
                libc::close(saved);
            }
        }
    }
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() {
        eprintln!("read stdin failed");
        return std::process::ExitCode::FAILURE;
    }
    let req: Request = match serde_json::from_str(&buf) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("parse request: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    // Stdout hygiene: blitz-html (under fulgur) println!s a bare
    // `ERROR: <html5ever parse error>` to *stdout* for every
    // non-fatal HTML parse hiccup — even pristine documents
    // trigger one. fulgur deliberately leaves the fd-1 redirect
    // to single-threaded callers (see fulgur's blitz_adapter
    // module docs); that's us. Park stdout on /dev/null for the
    // render so the noise can't interleave with PDF bytes in
    // stdout mode, then restore for the write. Real failures
    // still surface through the `Result`.
    let bytes = {
        let _gate = StdoutGate::divert();
        match req {
            Request::Invoice { data } => pdf::render_invoice(&data),
            Request::Html { html } => pdf::render_html(&html),
            Request::Template {
                name,
                template,
                data,
            } => pdf::render_template(&name, &template, &data),
        }
    };
    let bytes = match bytes {
        Ok(b) => b,
        Err(e) => {
            eprintln!("render: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match cli.out {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, &bytes) {
                eprintln!("write {}: {e}", path.display());
                return std::process::ExitCode::FAILURE;
            }
            eprintln!("wrote {} ({} bytes)", path.display(), bytes.len());
        }
        None => {
            let mut out = std::io::stdout().lock();
            if let Err(e) = out.write_all(&bytes) {
                eprintln!("write stdout: {e}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    std::process::ExitCode::SUCCESS
}
