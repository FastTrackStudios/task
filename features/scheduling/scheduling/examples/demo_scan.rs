//! `cargo run -p scheduling --example demo_scan` — sanity-check that
//! `examples/vault/scheduling/*` parses end-to-end with no surprises.
use std::path::PathBuf;

use scheduling::vault_scheduler::VaultScheduler;
use scheduling_proto::{Bookings, DayTemplates, EventTypes, Schedules};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "examples/vault".into()),
    );
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
