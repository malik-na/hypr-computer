//! The limits the user sets with `hypr-computer allow`, as the daemon goes by
//! them. They live in a file in the runtime dir, which the command writes;
//! the daemon reads it at every check, so a limit set mid-session applies to
//! the next call, one set while no daemon ran applies once one starts, and a
//! restart of the daemon doesn't lift them.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use domain::limits::{Action, Allow, Limits, Refusal};
use tracing::warn;

/// The most of the file read; a record is a few lines.
const MAX_RECORD: u64 = 1 << 20;

/// The limits file, `domain::paths::limits_file`, and the daemon's own
/// browser, which only open_url starts.
#[derive(Debug)]
pub struct LimitsFile {
    path: PathBuf,
    browser: Option<PathBuf>,
}

impl LimitsFile {
    /// The limits in `path`; `browser` is the browser the daemon was told to
    /// start, if it was told one.
    pub fn new(path: PathBuf, browser: Option<PathBuf>) -> Self {
        Self { path, browser }
    }

    /// Whether the limits allow `action` now; if not, why. Launching the
    /// daemon's own browser is refused like any browser, whatever its name:
    /// it would skip the proxy.
    pub fn check(&self, action: Action<'_>) -> Result<(), Refusal> {
        if let Action::Launch { program } = action
            && self.is_own_browser(program)
        {
            return Err(Refusal::Browser {
                program: program.rsplit('/').next().unwrap_or(program).to_owned(),
            });
        }
        let now = SystemTime::now();
        self.load(now).check(action, now)
    }

    /// Whether `program`, as launch_app would start it, is the daemon's
    /// browser: the same file, or one of the same name.
    fn is_own_browser(&self, program: &str) -> bool {
        let Some(browser) = &self.browser else {
            return false;
        };
        let same_name = Path::new(program).file_name().is_some()
            && Path::new(program).file_name() == browser.file_name();
        same_name
            || resolve(program)
                .is_some_and(|found| fs::canonicalize(found).ok() == fs::canonicalize(browser).ok())
    }

    /// The limits as the file has them at `now`. No file: no limits. A file
    /// that can't be read or trusted counts as read-only with no app and no
    /// site allowed, so that a damaged file never lifts a limit the user set.
    pub fn load(&self, now: SystemTime) -> Limits {
        let mut text = String::new();
        let read = File::open(&self.path)
            .and_then(|file| file.take(MAX_RECORD + 1).read_to_string(&mut text));
        match read {
            Ok(_) => Some(&text)
                .filter(|text| text.len() as u64 <= MAX_RECORD)
                .and_then(|text| Limits::from_record(text))
                .unwrap_or_else(|| {
                    warn!(
                        "the limits in {} are damaged; agents are read-only until the user runs \
                     `hypr-computer allow` again",
                        self.path.display()
                    );
                    nothing(now)
                }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Limits::none(),
            Err(e) => {
                warn!(
                    "can't read the limits in {} ({e}); agents are read-only until it can be read",
                    self.path.display()
                );
                nothing(now)
            }
        }
    }
}

/// The file `program` names, as starting it would find it: a path as it
/// is, a name on `PATH`.
fn resolve(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// Read-only, no app and no site: the most limits there are.
fn nothing(now: SystemTime) -> Limits {
    Limits::set(
        &Allow {
            apps: Some(BTreeSet::new()),
            sites: Some(BTreeSet::new()),
            read_only: true,
            duration: None,
        },
        now,
    )
}
