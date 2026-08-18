//! `cargo run -p scheduling --example demo_scan -- <vault>` —
//! sanity-check that a vault's `Projects/Scheduling/*` parses end to end
//! with no surprises.
//!
//! It used to default to `examples/vault`, a committed vault that no
//! longer exists — the wiki and scheduling fixtures it carried were the
//! last things addressing a vault by repo-relative path, and everything
//! else reaches a vault through an org. So the path is an argument now,
//! and a missing one says so rather than reporting four zeroes for a
//! directory that was never there.
use std::path::PathBuf;

use scheduling::vault_scheduler::VaultScheduler;
use scheduling_proto::{Bookings, DayTemplates, EventTypes, Schedules};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(arg) = std::env::args().nth(1) else {
        return Err("usage: demo_scan <vault-root> \
                    (e.g. a dev vault, or `~/.local/share/task-dev-seed/orgs/<slug>/vault`)"
            .into());
    };
    let root = PathBuf::from(arg);
    let s = VaultScheduler::new(root)?;
    let dts = s.list_day_templates()?;
    let ets = s.list_event_types()?;
    let scs = s.list_schedules()?;
    let bks = s.list_bookings()?;

    println!("day_templates: {}", dts.len());
    for d in &dts {
        println!("  - {} ({} blocks)", d.name, d.blocks.len());
    }
    println!("event_types: {}", ets.len());
    for e in &ets {
        println!(
            "  - {} ({}min, published={})",
            e.title, e.duration_min, e.published
        );
    }
    println!("schedules: {}", scs.len());
    for s in &scs {
        println!("  - {} ({} rules)", s.name, s.rules.len());
    }
    println!("bookings: {}", bks.len());
    for b in &bks {
        println!("  - {} @ {} [{:?}]", b.attendee_name, b.start_utc, b.status);
    }
    Ok(())
}
