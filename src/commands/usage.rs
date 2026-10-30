use crate::config;
use crate::usage::{self, UsageFile};
use anyhow::{Result, anyhow};
use std::fmt::Write as _;
use std::path::Path;
use vtui::usage::{RARELY, Report, Window};

/// Prints what the `[usage]` counts say about the days within `since`: plain text, or JSON with `json`.
pub fn run(since: Window, json: bool) -> Result<()> {
    let file = UsageFile::from_env().ok_or_else(|| anyhow!("no data folder to keep the usage counts in: set $PGBG_DATA_DIR"))?;
    let path = config::path();
    let is_counting = super::load(&path)?.counts_usage;
    let report = usage::rules().report(&file.read()?, chrono::Local::now().date_naive(), since);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", text(&report, since, (!is_counting).then_some(path.as_path())));
    }
    Ok(())
}

/// The report section by section, empty sections left out; `off_in` is the config to switch counting on in, when off.
fn text(report: &Report, since: Window, off_in: Option<&Path>) -> String {
    let mut out = String::new();
    let head = match since {
        Window::Days(days) => format!("the last {days} days"),
        Window::All => "every recorded day".to_owned(),
    };
    line(&mut out, &format!("pgbg usage · {head} · {} days counted", report.days));
    if let Some(config) = off_in {
        line(&mut out, &format!("counting is off: set `[usage] enabled = true` in {}", config.display()));
    }
    if report.days == 0 {
        return out;
    }
    line(&mut out, "\nNEVER USED");
    for unused in &report.never {
        line(&mut out, &format!("  {:<16}{}", unused.group, unused.actions.join("  ")));
    }
    if !report.rarely.is_empty() {
        line(&mut out, &format!("\nRARELY USED ({RARELY} times or less)"));
        let rarely: Vec<String> = report.rarely.iter().map(|count| format!("{} {}", count.action, count.count)).collect();
        line(&mut out, &format!("  {}", rarely.join(" · ")));
    }
    if !report.most.is_empty() {
        line(&mut out, "\nMOST USED");
        for count in &report.most {
            line(&mut out, &format!("  {:<22}{:>6}", count.action, count.count));
        }
    }
    if !report.screens.is_empty() {
        line(&mut out, "\nTIME");
        for time in &report.screens {
            line(&mut out, &format!("  {:<22}{:>6}", time.screen, vtui::usage::duration(time.seconds)));
        }
    }
    out
}

fn line(out: &mut String, text: &str) {
    #[expect(clippy::let_underscore_must_use, reason = "`writeln!` into a `String` cannot fail")]
    let _ = writeln!(out, "{text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Report {
        let days = usage::rules().parse(
            "{\"date\":\"2026-10-25\",\"actions\":{\"down\":40,\"chart\":2},\"screens\":{\"result\":4320}}\n\
             {\"date\":\"2026-10-26\",\"actions\":{\"down\":1,\"run\":12}}\n",
        );
        usage::rules().report(&days, "2026-10-26".parse().unwrap(), Window::Days(30))
    }

    #[test]
    fn the_report_reads_section_by_section() {
        let text = text(&report(), Window::Days(30), None);
        assert!(text.starts_with("pgbg usage · the last 30 days · 2 days counted\n"), "{text}");
        let order: Vec<Option<usize>> =
            ["NEVER USED", "RARELY USED", "MOST USED", "TIME"].iter().map(|s| text.find(&format!("\n{s}"))).collect();
        assert!(order.iter().all(Option::is_some) && order.is_sorted(), "{text}");
        assert!(text.contains("chart 2") && text.contains(&format!("{:<22}{:>6}", "down", 41)) && text.contains("1h 12m"), "{text}");
        assert!(!text.contains("counting is off"), "{text}");
    }

    #[test]
    fn no_day_counted_says_how_to_switch_it_on() {
        let empty = usage::rules().report(&[], "2026-10-26".parse().unwrap(), Window::All);
        let text = text(&empty, Window::All, Some(Path::new("/Users/nina/.config/pgbg/config.toml")));
        assert_eq!(
            text,
            "pgbg usage · every recorded day · 0 days counted\n\
             counting is off: set `[usage] enabled = true` in /Users/nina/.config/pgbg/config.toml\n"
        );
    }
}
