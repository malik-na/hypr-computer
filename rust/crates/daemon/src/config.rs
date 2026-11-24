//! The user's settings, in `$XDG_CONFIG_HOME/hypr-computer/config.toml`
//! (`domain::paths::config_file`), read when the daemon starts:
//!
//! ```toml
//! [log]
//! days = 3  # how long the action log is kept
//! ```
//!
//! A setting that is missing, or wrong, keeps its default; a wrong one is
//! said in the daemon's output.

use std::fs;
use std::io;
use std::path::Path;

use toml::{Table, Value};
use tracing::warn;

/// The settings in `[log]`.
const KNOWN: &[&str] = &["days"];

/// What the user set, or the defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// How many days an ended session's log is kept.
    pub days: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self { days: 3 }
    }
}

impl Settings {
    /// The settings in `file`; the defaults if there is none.
    pub fn read(file: &Path) -> Self {
        let text = match fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Self::default(),
            Err(e) => return Self::defaults_because(file, &e.to_string()),
        };
        match text.parse::<Table>() {
            Ok(table) => Self::from_table(file, &table),
            Err(e) => Self::defaults_because(file, &e.to_string()),
        }
    }

    fn from_table(file: &Path, table: &Table) -> Self {
        let mut settings = Self::default();
        let log = table.get("log").and_then(Value::as_table);
        if table.get("log").is_some() && log.is_none() {
            warn!("{}: `log` must be a [log] table", file.display());
        }
        for key in log.into_iter().flat_map(Table::keys) {
            if !KNOWN.contains(&key.as_str()) {
                warn!(
                    "{}: [log] has no setting {key:?}; it has {}",
                    file.display(),
                    KNOWN.join(", ")
                );
            }
        }
        if let Some(days) = log.and_then(|log| log.get("days")) {
            match days.as_integer().and_then(|d| u64::try_from(d).ok()) {
                Some(days) => settings.days = days,
                None => warn!(
                    "{}: [log] days must be a whole number of days, 0 or more, not {days}; \
                     keeping logs {} days",
                    file.display(),
                    settings.days
                ),
            }
        }
        settings
    }

    fn defaults_because(file: &Path, why: &str) -> Self {
        warn!("can't read {}, so the defaults hold: {why}", file.display());
        Self::default()
    }
}
