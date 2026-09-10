//! **The whole claim, as a program you can run.**
//!
//! Open an org, drive one service, print what came back. Nine lines of
//! actual work in `main`, and *none of them know which transport they
//! are on*. Change one environment variable and the same binary runs
//! against an in-process backend or a server across the internet.
//!
//! ```console
//! # in-process, over the local data root — no server anywhere
//! $ TASK_EMBED=1 cargo run -p task-client --example open_org -- acme-audio
//!
//! # against a running server — the demo one from `just demo serve`
//! $ TASK_VOX_URL=ws://127.0.0.1:18080 \
//!     cargo run -p task-client --example open_org -- acme-audio
//!
//! # against a deployment, using the token `task auth login` stored
//! $ TASK_VOX_URL=wss://task.starcommand.live \
//!     cargo run -p task-client --example open_org -- codywright
//! ```
//!
//! `just demo serve` plants `acme-audio` (see `examples/studio/`), so
//! the first two invocations above are runnable on a fresh checkout and
//! print the same projects.
//!
//! ## What this is proving
//!
//! A plugin — a backend feature compiled into the server — reaches
//! `ProjectService` by holding the backend and calling the trait. This
//! program holds `ProjectServiceClient` and calls the same methods.
//! Those are two implementations of one generated surface, so an
//! external app is not working with a reduced version of what a plugin
//! gets. That is the sentence the ADR's decision 4 is asking for, and
//! this file is the shortest honest demonstration of it.
//!
//! ## What it deliberately does not do
//!
//! There is no login flow here. Reaching a *remote* server as somebody
//! requires a session, and this example uses whatever `task auth login`
//! already stored. That is a real asymmetry with a plugin, which runs
//! inside the server's trust boundary and needs no credential at all —
//! and it is the honest place to leave it, because an external app
//! genuinely does need to authenticate and pretending otherwise would
//! be the papering-over the surface is supposed to avoid.

use project::ProjectServiceClient;
use task_client::TaskClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let slug = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "acme-audio".into());

    // Configuration decides the transport. Nothing below this line does.
    let task = TaskClient::from_env();
    println!(
        "opening `{slug}` — {}",
        if task.is_embedded() {
            "in-process".to_owned()
        } else {
            task.org_vox_url(&slug)
        }
    );

    let projects: ProjectServiceClient = task.org(&slug).await?;
    for p in projects.list().await? {
        println!("  {:<40} {}", p.title, p.status);
    }

    // A second service off the same client, to make the point that
    // `org()` is not a connection but a way of getting one: each call
    // establishes its own lane onto the same org.
    let more: ProjectServiceClient = task.org(&slug).await?;
    println!("{} projects", more.list().await?.len());
    Ok(())
}
