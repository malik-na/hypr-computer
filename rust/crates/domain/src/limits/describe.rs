//! The limits in words, for `hypr-computer allow --list`.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use super::Limits;

impl Limits {
    /// In words for the user: "No limits." when none is in force at `now`; otherwise each app, each site,
    /// "read-only" if so, and "until" with the time (UTC) if they have one.
    pub fn describe(&self, now: SystemTime) -> String {
        let Some(e) = self.in_effect(now) else {
            return "No limits: agents may use every app and site on their own screens, but never a \
                    password manager."
                .to_owned();
        };
        let mut lines = Vec::new();
        if e.read_only {
            lines.push(
                "Agents may only look at their screens (read-only): every tool but screenshot is \
                 refused."
                    .to_owned(),
            );
        }
        if let Some(apps) = &e.apps {
            lines.push(format!("Agents may use only these apps: {}.", join(apps)));
        }
        if let Some(sites) = &e.sites {
            lines.push(format!(
                "Their browsers may open only these sites, and their subdomains: {}.",
                join(sites)
            ));
        }
        if let Some(until) = self.until {
            let left = until.duration_since(now).unwrap_or_default();
            lines.push(format!(
                "These limits hold until {} ({} from now).",
                utc(until),
                span(left)
            ));
        }
        lines.join("\n")
    }
}

fn join(names: &BTreeSet<String>) -> String {
    if names.is_empty() {
        return "none".to_owned();
    }
    names.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// `2026-11-24 16:45:03 UTC`.
fn utc(time: SystemTime) -> String {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (days, of_day) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

/// The date `days` days after 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// `1h 29m 58s`, leaving out what is zero.
fn span(left: Duration) -> String {
    let secs = left.as_secs();
    let parts = [
        (secs / 3600, "h"),
        (secs % 3600 / 60, "m"),
        (secs % 60, "s"),
    ];
    let shown: Vec<String> = parts
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, unit)| format!("{n}{unit}"))
        .collect();
    if shown.is_empty() {
        "0s".to_owned()
    } else {
        shown.join(" ")
    }
}
