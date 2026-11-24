//! The limits as the file `hypr-computer allow` writes and the daemon reads at every check: a few lines of
//! text, so that the limits hold across a restart of the daemon and can be set while none runs.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use super::{Allow, Limits, normalize_site};

/// The record's first line; another version is not read.
const HEADER: &str = "hypr-computer limits 1";

impl Limits {
    /// The limits as text that [`Limits::from_record`] reads back the same.
    pub fn record(&self) -> String {
        let mut lines = vec![HEADER.to_owned()];
        if let Some(apps) = &self.allow.apps {
            lines.push("apps".to_owned());
            lines.extend(apps.iter().map(|app| format!("app {app}")));
        }
        if let Some(sites) = &self.allow.sites {
            lines.push("sites".to_owned());
            lines.extend(sites.iter().map(|site| format!("site {site}")));
        }
        if self.allow.read_only {
            lines.push("read-only".to_owned());
        }
        if let Some(until) = self.until {
            let at = until
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default();
            lines.push(format!("until {} {}", at.as_secs(), at.subsec_nanos()));
        }
        lines.push(String::new());
        lines.join("\n")
    }

    /// Reads a record [`Limits::record`] wrote. `None` for anything else, even a single odd line: the caller
    /// decides what a record it can't trust means.
    pub fn from_record(text: &str) -> Option<Limits> {
        let mut lines = text.strip_suffix('\n').unwrap_or(text).split('\n');
        if lines.next()? != HEADER {
            return None;
        }
        let mut allow = Allow::default();
        let mut until = None;
        for line in lines {
            let (word, value) = line.split_once(' ').unwrap_or((line, ""));
            match (word, value) {
                ("apps", "") if allow.apps.is_none() => allow.apps = Some(BTreeSet::new()),
                ("app", app) if is_name(app) => {
                    allow.apps.as_mut()?.insert(app.to_owned());
                }
                ("sites", "") if allow.sites.is_none() => allow.sites = Some(BTreeSet::new()),
                ("site", site) if normalize_site(site).as_deref() == Some(site) => {
                    allow.sites.as_mut()?.insert(site.to_owned());
                }
                ("read-only", "") if !allow.read_only => allow.read_only = true,
                ("until", at) if until.is_none() => {
                    let (secs, nanos) = at.split_once(' ')?;
                    let nanos: u32 = nanos.parse().ok().filter(|n| *n < 1_000_000_000)?;
                    let at = Duration::new(secs.parse().ok()?, nanos);
                    until = Some(SystemTime::UNIX_EPOCH.checked_add(at)?);
                }
                _ => return None,
            }
        }
        if !allow.limits_something() {
            return (until.is_none()).then(Limits::none);
        }
        Some(Limits { allow, until })
    }
}

/// An app's name as `allow` takes it: lower case, no `/`, nothing unprintable.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.chars().any(char::is_control)
        && name.to_lowercase() == name
}
