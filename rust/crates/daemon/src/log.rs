//! The action log: every tool call of every session, one JSON line each, in
//! a file of the session's own under `<state>/hypr-computer/log/`. It is the
//! user's alone: the daemon makes every dir in it 0700 and every file 0600
//! itself, whatever its umask. A record holds what the guard says of a call
//! (`tools::Call`): its arguments with anything secret already replaced by
//! its length (`Tool::logged`), and a result word, never the reply's text,
//! which can echo the arguments.

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use domain::SessionId;
use domain::paths::LOG_NAME;
use serde_json::{Value, json};
use tools::Call;
use tracing::warn;

mod show;

/// The log of every session, this daemon's and earlier ones'.
pub struct ActionLog {
    /// `<state>/hypr-computer`.
    state: PathBuf,
    /// `<state>/hypr-computer/log`.
    dir: PathBuf,
    /// This daemon's run. Its sessions' files are named after it, since
    /// session numbers start again at 1 with each daemon.
    run: String,
}

impl ActionLog {
    /// The log in `state` (`domain::paths::state_dir`). Makes its dirs, and
    /// closes up whatever in it was opened to others.
    pub fn open(state: PathBuf) -> io::Result<Self> {
        let log = Self {
            dir: state.join(LOG_NAME),
            state,
            run: format!(
                "{}-{}",
                utc(SystemTime::now()).replace(':', ""),
                std::process::id()
            ),
        };
        log.private_dirs()?;
        close_up(&log.state);
        Ok(log)
    }

    /// The log of this daemon's session `id`.
    pub fn session(self: &Arc<Self>, id: SessionId) -> SessionLog {
        SessionLog {
            log: self.clone(),
            session: id.get(),
            file: self.file(id.get()),
            agent: OnceLock::new(),
        }
    }

    /// The file of this daemon's session `n`.
    fn file(&self, n: u64) -> PathBuf {
        self.dir.join(format!("{}.session-{n}.jsonl", self.run))
    }

    /// Makes the state dir and the log dir, the user's only.
    fn private_dirs(&self) -> io::Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)?;
        for dir in [&self.state, &self.dir] {
            make_private(dir, 0o700)?;
        }
        Ok(())
    }
}

/// One session's log.
pub struct SessionLog {
    log: Arc<ActionLog>,
    session: u64,
    file: PathBuf,
    /// The name the agent gave, once it has.
    agent: OnceLock<String>,
}

impl SessionLog {
    /// The agent said its name.
    pub fn named(&self, agent: &str) {
        let _ = self.agent.set(agent.to_owned());
    }

    /// Adds `call` to the log. A log that can't be written is said in the
    /// daemon's output, by its path only.
    pub fn record(&self, call: &Call<'_>) {
        let mut record = json!({
            "time": utc(call.time),
            "session": self.session,
            "agent": self.agent.get().map_or("", String::as_str),
            "tool": call.tool,
            "result": call.result.word(),
        });
        if let Some(arguments) = &call.arguments {
            record["arguments"] = Value::Object(arguments.clone());
        }
        if let Err(e) = self.append(&record) {
            warn!("can't write the action log {}: {e}", self.file.display());
        }
    }

    fn append(&self, record: &Value) -> io::Result<()> {
        self.log.private_dirs()?;
        let mut file = private_file(&self.file)?;
        file.write_all(format!("{record}\n").as_bytes())
    }
}

/// Opens `path` to add to it, made if need be, the user's only.
fn private_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    if file.metadata()?.permissions().mode() & 0o7777 != 0o600 {
        file.set_permissions(Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

/// Sets `path`'s mode to `mode` unless it is already.
fn make_private(path: &Path, mode: u32) -> io::Result<()> {
    if fs::symlink_metadata(path)?.permissions().mode() & 0o7777 != mode {
        fs::set_permissions(path, Permissions::from_mode(mode))?;
    }
    Ok(())
}

/// Makes every dir under `root` 0700 and every file 0600, `root` included.
/// Symlinks are left as they are, and not followed.
fn close_up(root: &Path) {
    let Ok(meta) = fs::symlink_metadata(root) else {
        return;
    };
    let mode = if meta.is_dir() {
        0o700
    } else if meta.is_file() {
        0o600
    } else {
        return;
    };
    if let Err(e) = make_private(root, mode) {
        warn!("can't make {} private: {e}", root.display());
    }
    if meta.is_dir()
        && let Ok(entries) = fs::read_dir(root)
    {
        for entry in entries.flatten() {
            close_up(&entry.path());
        }
    }
}

/// `time` in RFC 3339, in UTC to the millisecond: `2026-11-24T10:15:03.042Z`.
fn utc(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let (days, secs) = (since.as_secs() / 86_400, since.as_secs() % 86_400);
    let (year, month, day) = civil_date(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60,
        since.subsec_millis()
    )
}

/// The date `days` after 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_date(days: u64) -> (u64, u64, u64) {
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
