//! `task cycle …` — the cyclic life-calendar surface.
//!
//! `current` / `list` are pure computation (no data at all).
//! `reflect` writes an org WIKI page and goes through the wiki
//! Pages service over vox — remote server or embedded backend
//! alike.

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum CycleCmd {
    /// Show the cycle that today's date sits inside. Prints
    /// year / quarter / cycle ordinal + cycle bounds + how
    /// far through it we are. Returns "(reset / bonus week)"
    /// when today is between cycles.
    Current {
        /// Emit the cycle (+ derived progress) as JSON.
        /// `{"cycle": null, …}` between cycles.
        #[arg(long)]
        json: bool,
    },
    /// List every quarter + cycle for a given cyclic year.
    /// Defaults to the current calendar year.
    List {
        #[arg(long)]
        year: Option<i32>,
        /// Week-start day. Default: Monday.
        #[arg(long, default_value = "mon")]
        week_start: String,
        /// Emit the year's quarters + cycles as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Capture a reflection note for a cycle. Writes a
    /// templated page at
    /// `<org>/wiki/Knowledge/cycles/<year>-Q<q>-C<n>.md`.
    /// Idempotent: if the file exists, prints its path
    /// instead of overwriting.
    ///
    /// Defaults to today's cycle (or the previous one when
    /// today is inside a reset week). Override with
    /// `--year/--quarter/--cycle`.
    Reflect {
        #[arg(long)]
        year: Option<i32>,
        #[arg(long)]
        quarter: Option<u8>,
        #[arg(long)]
        cycle: Option<u8>,
    },
}

pub(crate) async fn run_cycle(cmd: CycleCmd) -> eyre::Result<()> {
    use chrono::Datelike;
    let rule = cycle::FirstWeekRule::AtLeastFourDaysInYear;
    match cmd {
        CycleCmd::Current { json } => {
            let today = chrono::Local::now().date_naive();
            // Walk the year (and its neighbors) to find whether
            // we're inside a cycle or in a reset / bonus week.
            if let Some(c) = cycle::cycle_for_date(today, chrono::Weekday::Mon, rule) {
                if json {
                    crate::json_out::print_json(&crate::json_out::cycle_json(&c, today))?;
                    return Ok(());
                }
                let total = (c.end_date - c.start_date).num_days() + 1;
                let elapsed = (today - c.start_date).num_days() + 1;
                let pct = (elapsed as f64) * 100.0 / (total as f64);
                println!(
                    "{}-Q{}-C{}  ({} → {})",
                    c.year, c.quarter, c.ordinal, c.start_date, c.end_date,
                );
                println!("today:   {today}");
                println!("day {elapsed} of {total}   ({pct:.0}%)");
                println!("id:      {}", c.id);
            } else if json {
                crate::json_out::print_json(&serde_json::json!({
                    "cycle": null,
                    "today": today,
                    "between_cycles": true,
                }))?;
            } else {
                println!("today ({today}) is between cycles — reset or bonus week");
            }
        }
        CycleCmd::List {
            year,
            week_start,
            json,
        } => {
            let year = year.unwrap_or_else(|| chrono::Local::now().year());
            let wd = cycle::weekday_from_short(&week_start)
                .ok_or_else(|| eyre::eyre!("bad --week-start `{week_start}`"))?;
            let qs = cycle::generate_year(year, wd, rule);
            let bonus = cycle::has_bonus_week(year, wd, rule);
            if json {
                crate::json_out::print_json(&serde_json::json!({
                    "year": year,
                    "week_start": week_start,
                    "bonus_week": bonus,
                    "quarters": qs,
                }))?;
                return Ok(());
            }
            println!(
                "Cyclic year {year}  week-start={week_start}  {}",
                if bonus { "[cyclic-leap]" } else { "" }
            );
            for q in qs {
                println!("\nQ{}  {} → {}", q.ordinal, q.start_date, q.end_date,);
                for c in q.cycles.iter() {
                    println!(
                        "  C{}   {} → {}   ({} days)",
                        c.ordinal,
                        c.start_date,
                        c.end_date,
                        (c.end_date - c.start_date).num_days() + 1,
                    );
                }
                println!("  reset  {} → {}", q.reset_week_start, q.reset_week_end,);
                if let (Some(s), Some(e)) = (q.bonus_week_start, q.bonus_week_end) {
                    println!("  bonus  {s} → {e}   (week zero for {})", year + 1);
                }
            }
        }
        CycleCmd::Reflect {
            year,
            quarter,
            cycle,
        } => {
            let target = pick_reflection_cycle(year, quarter, cycle, rule)
                .ok_or_else(|| eyre::eyre!("no matching cycle"))?;
            // Reflections are wiki pages (`wiki/Knowledge/cycles/…`)
            // — org DATA, written through the wiki Pages service so
            // the command works against a remote org too (and the
            // org router's gates apply). Display the on-disk path
            // when the org lives on this machine, else the
            // wiki-relative path.
            let slug = crate::resolve_slug(None)?;
            let filename = format!("{}-Q{}-C{}.md", target.year, target.quarter, target.ordinal);
            let rel = format!("cycles/{filename}");
            let display = org_proto::DataRoot::from_env().ok().map_or_else(
                || rel.clone(),
                |r| {
                    r.org(&slug)
                        .wiki_knowledge_dir()
                        .join(&rel)
                        .display()
                        .to_string()
                },
            );
            let url = crate::resolve_org_vox_url(None, &slug);
            let pages: wiki_proto::service::pages::PagesClient =
                crate::establish_for_url(&url).await?;
            if pages
                .read_page(org_proto::DEFAULT_WIKI.to_owned(), rel.clone())
                .await
                .is_ok()
            {
                println!("(reflection already exists)");
                println!("  {display}");
                return Ok(());
            }
            let now = chrono::Utc::now();
            let body = format!(
                "---\n\
                 type: cycle-reflection\n\
                 id: {id}\n\
                 cycleId: {id}\n\
                 year: {year}\n\
                 quarter: {quarter}\n\
                 cycle: {ordinal}\n\
                 start: {start}\n\
                 end: {end}\n\
                 dateCreated: {created}\n\
                 ---\n\
                 \n\
                 # {year} Q{quarter} C{ordinal} reflection\n\
                 \n\
                 Cycle window: **{start} → {end}** (4 weeks, 25% each).\n\
                 \n\
                 ## What worked\n\
                 \n\
                 - \n\
                 \n\
                 ## What didn't\n\
                 \n\
                 - \n\
                 \n\
                 ## Lessons\n\
                 \n\
                 - \n\
                 \n\
                 ## Going into the next cycle\n\
                 \n\
                 - \n",
                id = target.id,
                year = target.year,
                quarter = target.quarter,
                ordinal = target.ordinal,
                start = target.start_date,
                end = target.end_date,
                created = now.to_rfc3339(),
            );
            pages
                .write_page(
                    org_proto::DEFAULT_WIKI.to_owned(),
                    rel.clone(),
                    body,
                    String::new(),
                )
                .await
                .map_err(|e| eyre::eyre!("write {rel}: {e:?}"))?;
            println!("Created cycle reflection at:");
            println!("  {display}");
            println!(
                "  for {}-Q{}-C{} ({} → {})",
                target.year, target.quarter, target.ordinal, target.start_date, target.end_date,
            );
        }
    }
    Ok(())
}

/// Resolve which `cycle::Cycle` a reflection should target. If any
/// of (year, quarter, cycle) are explicit, walk the generator and
/// look it up. Otherwise pick the cycle that today's date lands in;
/// if today is in a reset week, pick the cycle that just ended
/// (the most recent C3 of that quarter).
fn pick_reflection_cycle(
    year: Option<i32>,
    quarter: Option<u8>,
    cycle_ord: Option<u8>,
    rule: cycle::FirstWeekRule,
) -> Option<cycle::Cycle> {
    use chrono::Datelike;
    let today = chrono::Local::now().date_naive();
    if let (Some(y), Some(q), Some(c)) = (year, quarter, cycle_ord) {
        let quarters = cycle::generate_year(y, chrono::Weekday::Mon, rule);
        let qrec = quarters.iter().find(|qr| qr.ordinal == q)?;
        return qrec.cycles.iter().find(|cy| cy.ordinal == c).cloned();
    }
    if let Some(c) = cycle::cycle_for_date(today, chrono::Weekday::Mon, rule) {
        return Some(c);
    }
    // Reset week → walk this year's quarters and grab the most
    // recent C3 that ended before today.
    for cyclic_year in [today.year(), today.year() - 1] {
        let quarters = cycle::generate_year(cyclic_year, chrono::Weekday::Mon, rule);
        let mut latest: Option<cycle::Cycle> = None;
        for q in quarters {
            for c in q.cycles.iter() {
                if c.end_date < today && latest.as_ref().is_none_or(|l| c.end_date > l.end_date) {
                    latest = Some(c.clone());
                }
            }
        }
        if latest.is_some() {
            return latest;
        }
    }
    None
}
