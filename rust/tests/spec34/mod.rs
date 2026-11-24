//! Helpers for the black-box tests of #34 ("Action log"), written from the
//! spec by an independent test agent.
//!
//! They drive the real daemon, shim and screen as agents do (MCP through the
//! shim), and the `hypr-computer` command as the user runs it, each test in a
//! throwaway home and runtime dir under /tmp with a cleared environment, so
//! nothing can reach the desktop, or the log, of the machine running them.
//! Agents, their probes and stopping come from #28's helpers (`spec28/`).
//!
//! # The interface these tests assume
//!
//! **Where.** The log lives in `$XDG_STATE_HOME/hypr-computer/log/`, or
//! `~/.local/state/hypr-computer/log/` when `$XDG_STATE_HOME` is unset or
//! empty. Every directory the daemon makes or uses from
//! `<state>/hypr-computer/` down is mode 0700 (one that already exists with
//! a looser mode is tightened), and every file in it is mode 0600, whatever
//! the daemon's umask.
//!
//! **What.** Every tool call an agent makes is logged: ones that worked,
//! failed, were refused (screen still starting, stopped by the user) or were
//! invalid (unknown tool, bad arguments). Each session's records go to one
//! file of its own, `<something>.jsonl` (the name is the implementation's),
//! anywhere under `log/`, one JSON object per line, in call order:
//!
//! ```json
//! {"time": "2026-11-24T10:15:03.042Z", "session": 1, "agent": "alpha",
//!  "tool": "click", "arguments": {"x": 640, "y": 400}, "result": "ok"}
//! ```
//!
//! - `time`: when the call came in, RFC 3339 in UTC with a `Z`.
//! - `session`: the session number, as on the agent's live view (numbers
//!   start again at 1 each time the daemon starts).
//! - `agent`: the name its MCP client gave, `""` if none.
//! - `tool`: the tool's name.
//! - `arguments`: the call's arguments as given, except:
//!   - `type`: `text` is recorded as `{"characters": N}` (Unicode scalar
//!     values), never its content;
//!   - `key`: a press that only types a character, a single printable key
//!     alone or with Shift (`z`, `shift+q`, `7`), is recorded as
//!     `{"keys": {"characters": 1}}`; anything else (`ctrl+l`, `Return`,
//!     `alt+Left`, `F5`) in full;
//!   - `open_url`: `url` is recorded as its origin only,
//!     `scheme://host[:port]`: user info, path, query and fragment are
//!     dropped, since they can hold secrets (reset links, tokens). A `url`
//!     that doesn't parse is recorded as `{"characters": N}`;
//!   - an unknown tool's arguments are not recorded.
//! - `result`: `"ok"`, `"error"` (the tool failed, or the call was refused
//!   for a reason other than a stop), `"stopped"` (refused or ended by a
//!   user's stop) or `"invalid"` (unknown tool or bad arguments). Nothing
//!   else from the reply is recorded: error texts can echo arguments.
//! - `screenshot` (only with screenshots on, see below): where the image the
//!   agent got is saved, relative to `log/`.
//!
//! A call's record is written by the time the agent has its reply (the tests
//! allow a moment). Nothing the agent typed, nor a dropped part of a URL,
//! reaches the disk anywhere, the daemon's own output (the journal) included.
//!
//! **Settings** are in `$XDG_CONFIG_HOME/hypr-computer/config.toml` (or
//! `~/.config/hypr-computer/config.toml`), read when the daemon starts:
//!
//! ```toml
//! [log]
//! days = 3            # how long logs are kept; the default is 3
//! screenshots = false # whether the images agents get are saved; off by default
//! ```
//!
//! **Retention.** A session's log (its `.jsonl` file and its screenshots) is
//! removed once the session has ended and its `.jsonl` file was last
//! modified more than `days` days ago. The daemon removes them when it
//! starts and each time an agent connects. A running session's log is never
//! removed. The tests make logs old by setting their modification time back,
//! never by waiting.
//!
//! **Screenshots.** With `screenshots = true`, each image a tool gives the
//! agent (a `screenshot` call's picture) is saved as a PNG file under `log/`,
//! exactly as the agent got it, and its record names the file. With the
//! setting off, no image reaches the disk in any form.
//!
//! **`hypr-computer log <agent>`** shows the log of the agents of that name
//! (every session of that name still in the log, oldest first, the running
//! daemon's and earlier ones), or of that session number of the running
//! daemon, as `stop <agent>` picks agents. Each session starts with a line
//! naming it (`alpha (session 1)`, with when it started); then one line per
//! call, in order: its time, the tool, its arguments as recorded (typed text
//! as `<N> characters`), and its result word (`ok`, `error`, `stopped`,
//! `invalid`), and the absolute path of its saved screenshot, if any. It exits 0. An
//! agent with nothing in the log is an error: non-zero, saying so on stderr.
//!
//! # No TLA+ spec
//!
//! The log is a write-only side effect of the guard plus a sweep, with no
//! protocol of its own: no state another part waits on, no take-over or
//! hand-back. Its one ordering hazard, the sweep removing a log a running
//! session still writes to, is pinned by a black-box test
//! (`a_running_agents_log_is_never_removed`), so #39's "a spec per stateful
//! part" isn't applied here.

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

#[path = "../spec28/mod.rs"]
pub mod spec28;

use std::ffi::OsString;
use std::fs::{self, FileTimes};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

pub use spec28::probe;
// Each test file uses a different subset of these too.
#[allow(unused_imports)]
pub use spec28::{Agent, Env, Image, Ran, SLOW, Shim, TIMEOUT, assert_stopped, bin, wait_until};

/// A day, for making logs old.
pub const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// How a test's daemon is set up.
#[derive(Clone, Debug, Default)]
pub struct Setup {
    /// The contents of the settings file, if there is one.
    pub config: Option<String>,
    /// Set `$XDG_STATE_HOME` and `$XDG_CONFIG_HOME` (to dirs other than the
    /// defaults under the home) for the daemon and the command.
    pub xdg_dirs: bool,
    /// Extra environment for the daemon.
    pub daemon_env: Vec<(String, OsString)>,
}

/// A daemon (restartable), and the agents that connect to it.
pub struct World {
    // Dropped first: the daemon stops before its dirs go.
    daemon: Option<Daemon>,
    pub env: Env,
    setup: Setup,
    connected: u64,
    runs: u32,
}

impl World {
    /// The default setup: no settings file, no XDG dirs but the runtime dir.
    pub fn new() -> Self {
        Self::with(Setup::default())
    }

    /// A daemon with this settings file.
    pub fn with_config(config: &str) -> Self {
        Self::with(Setup {
            config: Some(config.to_owned()),
            ..Setup::default()
        })
    }

    pub fn with(setup: Setup) -> Self {
        let env = Env::new();
        let mut world = World {
            daemon: None,
            env,
            setup,
            connected: 0,
            runs: 0,
        };
        if let Some(config) = world.setup.config.clone() {
            let file = world.config_file();
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, config).unwrap();
        }
        world.start_daemon();
        world
    }

    fn start_daemon(&mut self) {
        self.runs += 1;
        let log = self.env.path(&format!("daemon-{}.log", self.runs));
        self.daemon = Some(Daemon::start(
            &self.env,
            &self.xdg_env(),
            &self.setup.daemon_env,
            log,
        ));
        self.connected = 0;
    }

    /// Stops the daemon as systemd would, and starts a new one on the same
    /// home and runtime dir. Session numbers start again at 1.
    pub fn restart_daemon(&mut self) {
        self.daemon = None;
        self.start_daemon();
    }

    /// Everything the daemons have printed (what the journal would hold).
    pub fn daemon_output(&self) -> String {
        (1..=self.runs)
            .map(|n| {
                fs::read_to_string(self.env.path(&format!("daemon-{n}.log"))).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn xdg_env(&self) -> Vec<(String, OsString)> {
        if self.setup.xdg_dirs {
            vec![
                ("XDG_STATE_HOME".into(), self.env.path("xdg-state").into()),
                ("XDG_CONFIG_HOME".into(), self.env.path("xdg-config").into()),
            ]
        } else {
            Vec::new()
        }
    }

    pub fn home(&self) -> PathBuf {
        self.env.path("home")
    }

    /// Where the settings file is, for this setup.
    pub fn config_file(&self) -> PathBuf {
        let base = if self.setup.xdg_dirs {
            self.env.path("xdg-config")
        } else {
            self.home().join(".config")
        };
        base.join("hypr-computer").join("config.toml")
    }

    /// `<state>/hypr-computer`, for this setup.
    pub fn state_dir(&self) -> PathBuf {
        let base = if self.setup.xdg_dirs {
            self.env.path("xdg-state")
        } else {
            self.home().join(".local").join("state")
        };
        base.join("hypr-computer")
    }

    pub fn log_dir(&self) -> PathBuf {
        self.state_dir().join("log")
    }

    /// The test's whole throwaway dir: home, runtime dir, XDG dirs and the
    /// daemon's output.
    pub fn everything(&self) -> PathBuf {
        self.env.path("")
    }

    /// An agent called `name` connects. Returns its shim and session number,
    /// without waiting for its screen.
    pub fn connect(&mut self, name: &str) -> (Shim, u64) {
        let shim = Shim::start(&self.env, name);
        self.connected += 1;
        (shim, self.connected)
    }

    /// An agent called `name`, its screen ready, with the probe on it.
    pub fn agent(&mut self, name: &str) -> Agent {
        self.agent_with(name, Duration::ZERO)
    }

    /// Like [`World::agent`], with a slow app on its screen (`spec28::SLOW`).
    pub fn slow_agent(&mut self, name: &str) -> Agent {
        self.agent_with(name, SLOW)
    }

    fn agent_with(&mut self, name: &str, pong_delay: Duration) -> Agent {
        let (mut shim, session) = self.connect(name);
        shim.screenshot();
        let probe = probe::Probe::start_slow(&self.env.screen_socket(session), pong_delay);
        Agent {
            probe,
            shim,
            session,
            name: name.to_owned(),
        }
    }

    /// `hypr-computer <args>`, whatever happens.
    pub fn hypr_computer(&self, args: &[&str]) -> Ran {
        let mut cmd = self.env.command(&bin("hypr-computer"));
        for (k, v) in self.xdg_env() {
            cmd.env(k, v);
        }
        run_to_end(cmd.args(args), args)
    }

    /// `hypr-computer <args>`, which must succeed.
    #[track_caller]
    pub fn user_runs(&self, args: &[&str]) -> Ran {
        let ran = self.hypr_computer(args);
        assert!(
            ran.success,
            "`hypr-computer {}` failed: {ran:?}",
            args.join(" ")
        );
        ran
    }

    /// Every record in the log, file by file, in each file's order.
    pub fn records(&self) -> Vec<Record> {
        let mut out = Vec::new();
        for file in files_under(&self.log_dir()) {
            if file.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let text = fs::read_to_string(&file).unwrap_or_default();
            // The last line may be half written.
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                if let Ok(value) = serde_json::from_str::<Value>(line) {
                    out.push(Record {
                        file: file.clone(),
                        value,
                    });
                }
            }
        }
        out
    }

    /// The records of session `n` (of the running daemon), in order.
    pub fn session_records(&self, n: u64) -> Vec<Record> {
        self.records()
            .into_iter()
            .filter(|r| r.session() == n)
            .collect()
    }

    /// Waits until session `n` has at least `count` records, and returns
    /// them; fails the test if it never has.
    #[track_caller]
    pub fn wait_records(&self, n: u64, count: usize) -> Vec<Record> {
        let mut got = Vec::new();
        let ok = wait_until(TIMEOUT, || {
            got = self.session_records(n);
            got.len() >= count
        });
        assert!(
            ok,
            "session {n} must have at least {count} records in the log under {}; it has {}: {:#?}\n\
             files there: {:?}",
            self.log_dir().display(),
            got.len(),
            got,
            files_under(&self.state_dir())
        );
        got
    }

    /// How many records session `n` has once [`World::agent`] is done: its
    /// screenshots while its screen started, the last one working. Waits
    /// for that last one to be logged.
    #[track_caller]
    pub fn after_setup(&self, n: u64) -> usize {
        let mut count = 0;
        let ok = wait_until(TIMEOUT, || {
            let records = self.session_records(n);
            count = records.len();
            records
                .last()
                .is_some_and(|r| r.tool() == "screenshot" && r.result() == "ok")
        });
        assert!(
            ok,
            "the screenshot that found session {n}'s screen ready must be logged as ok; \
             its records: {:#?}",
            self.session_records(n)
        );
        count
    }

    /// The `.jsonl` file holding session `n`'s records; there must be one.
    #[track_caller]
    pub fn session_file(&self, n: u64) -> PathBuf {
        let records = self.wait_records(n, 1);
        let files: Vec<&PathBuf> = records.iter().map(|r| &r.file).collect();
        assert!(
            files.iter().all(|f| *f == files[0]),
            "session {n}'s records must all be in one file: {files:?}"
        );
        files[0].clone()
    }
}

/// Runs a command to its end (within [`TIMEOUT`]) and returns what it said.
fn run_to_end(cmd: &mut Command, args: &[&str]) -> Ran {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let end = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            panic!("hypr-computer {args:?} didn't finish within {TIMEOUT:?}");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let (mut stdout, mut stderr) = (String::new(), String::new());
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    Ran {
        success: status.success(),
        stdout,
        stderr,
    }
}

/// The real daemon, started under `umask 000` (so modes it doesn't set
/// itself would show), its stderr going to a file.
pub struct Daemon {
    child: Child,
    log: PathBuf,
}

impl Daemon {
    fn start(
        env: &Env,
        xdg: &[(String, OsString)],
        extra: &[(String, OsString)],
        log: PathBuf,
    ) -> Self {
        let mut cmd = env.command(Path::new("/bin/sh"));
        cmd.args(["-c", "umask 000; exec \"$0\""])
            .arg(bin("hypr-computer-daemon"));
        for (k, v) in xdg.iter().chain(extra) {
            cmd.env(k, v);
        }
        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        let mut daemon = Daemon { child, log };
        let ok = wait_until(TIMEOUT, || {
            daemon.output().contains("listening on") || !daemon.is_running()
        });
        assert!(
            ok && daemon.is_running(),
            "the daemon didn't start:\n{}",
            daemon.output()
        );
        daemon
    }

    pub fn output(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }
}

impl Drop for Daemon {
    /// Stops the daemon as systemd would, and kills it only if it won't go.
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        if !wait_until(Duration::from_secs(5), || !self.is_running()) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// One line of a session's log.
#[derive(Clone, Debug)]
pub struct Record {
    pub file: PathBuf,
    pub value: Value,
}

impl Record {
    pub fn session(&self) -> u64 {
        self.value["session"].as_u64().unwrap_or(0)
    }
    pub fn agent(&self) -> &str {
        self.value["agent"].as_str().unwrap_or("")
    }
    pub fn tool(&self) -> &str {
        self.value["tool"].as_str().unwrap_or("")
    }
    pub fn arguments(&self) -> &Value {
        &self.value["arguments"]
    }
    pub fn result(&self) -> &str {
        self.value["result"].as_str().unwrap_or("")
    }
    /// When the call came in; fails the test unless it is RFC 3339 in UTC.
    #[track_caller]
    pub fn time(&self) -> SystemTime {
        let text = self.value["time"].as_str().unwrap_or("");
        parse_utc(text).unwrap_or_else(|| {
            panic!(
                "a record's time must be RFC 3339 in UTC, like 2026-11-24T10:15:03.042Z: {}",
                self.value
            )
        })
    }
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.frac]Z`.
pub fn parse_utc(text: &str) -> Option<SystemTime> {
    let text = text.strip_suffix('Z')?;
    let (date, clock) = text.split_once('T')?;
    let d: Vec<i64> = date
        .split('-')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let (whole, frac) = clock.split_once('.').unwrap_or((clock, ""));
    let t: Vec<u64> = whole
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    if d.len() != 3 || t.len() != 3 || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let nanos: u32 = if frac.is_empty() {
        0
    } else {
        format!("{:0<9}", &frac[..frac.len().min(9)]).parse().ok()?
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let (y, m, day) = (d[0] - i64::from(d[1] <= 2), d[1], d[2]);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = u64::try_from(era * 146_097 + doe - 719_468).ok()?;
    let secs = days * 86_400 + t[0] * 3600 + t[1] * 60 + t[2];
    Some(UNIX_EPOCH + Duration::new(secs, nanos))
}

/// Every regular file under `root`, not following symlinks; none if it
/// doesn't exist.
pub fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, &mut |p, meta| {
        if meta.file_type().is_file() {
            out.push(p.to_path_buf());
        }
    });
    out.sort();
    out
}

/// Calls `f` on `root` and everything under it, not following symlinks.
pub fn walk(root: &Path, f: &mut dyn FnMut(&Path, &fs::Metadata)) {
    let Ok(meta) = fs::symlink_metadata(root) else {
        return;
    };
    f(root, &meta);
    if meta.is_dir()
        && let Ok(entries) = fs::read_dir(root)
    {
        for entry in entries.flatten() {
            walk(&entry.path(), f);
        }
    }
}

/// The files under `root` whose bytes contain `needle`.
pub fn files_containing(root: &Path, needle: &[u8]) -> Vec<PathBuf> {
    files_under(root)
        .into_iter()
        .filter(|f| fs::read(f).is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle)))
        .collect()
}

/// Fails the test if `needle` is in any file under `root`, or in `extra`
/// (the daemon's output).
#[track_caller]
pub fn assert_nowhere(root: &Path, needle: &str, extra: &str, what: &str) {
    let found = files_containing(root, needle.as_bytes());
    assert!(
        found.is_empty(),
        "{what} ({needle:?}) reached the disk, in {found:?}"
    );
    assert!(
        !extra.contains(needle),
        "{what} ({needle:?}) is in the daemon's output (the journal)"
    );
}

/// Each entry under `root` (itself included) whose mode isn't 0700 for a
/// directory or 0600 for a file, or that is something else, or isn't the
/// user's; with what it is.
pub fn loose_entries(root: &Path) -> Vec<String> {
    let me = fs::metadata(root.parent().unwrap_or(root)).map_or(u32::MAX, |m| m.uid());
    let mut bad = Vec::new();
    walk(root, &mut |p, meta| {
        let mode = meta.permissions().mode() & 0o7777;
        let kind = meta.file_type();
        let ok = (kind.is_dir() && mode == 0o700) || (kind.is_file() && mode == 0o600);
        if !ok || meta.uid() != me {
            bad.push(format!(
                "{} ({}, mode {mode:04o}, uid {})",
                p.display(),
                if kind.is_dir() {
                    "dir"
                } else if kind.is_file() {
                    "file"
                } else {
                    "other"
                },
                meta.uid()
            ));
        }
    });
    bad
}

/// Sets the modification (and access) time of `path`, and of everything
/// under it, `age` into the past.
pub fn backdate(path: &Path, age: Duration) {
    let when = SystemTime::now() - age;
    let mut paths = Vec::new();
    walk(path, &mut |p, _| paths.push(p.to_path_buf()));
    // Deepest first, so setting a file's time doesn't touch its dir's after.
    for p in paths.iter().rev() {
        let file = fs::File::options()
            .write(fs::symlink_metadata(p).is_ok_and(|m| m.is_file()))
            .read(true)
            .open(p)
            .unwrap();
        file.set_times(FileTimes::new().set_accessed(when).set_modified(when))
            .unwrap();
    }
}

/// Whether these bytes are a PNG file.
pub fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

/// Every PNG under `root`, by content, and every file holding a PNG in
/// base64 (as a JSON log line would), whatever their names.
pub fn pictures_under(root: &Path) -> Vec<PathBuf> {
    files_under(root)
        .into_iter()
        .filter(|f| {
            fs::read(f).is_ok_and(|b| is_png(&b) || b.windows(11).any(|w| w == b"iVBORw0KGgo"))
        })
        .collect()
}

/// Decodes a PNG file as a screenshot reply would carry it.
pub fn image_file(path: &Path) -> Image {
    let data = STANDARD.encode(fs::read(path).unwrap());
    Image::from_result(
        &json!({"content": [{"type": "image", "mimeType": "image/png", "data": data}]}),
    )
}

/// The lines of `text` that hold `word` as a word of their own.
pub fn lines_with<'a>(text: &'a str, word: &str) -> Vec<&'a str> {
    text.lines()
        .filter(|l| {
            l.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|w| w == word)
        })
        .collect()
}

/// Whether `line` holds a time of day, `HH:MM:SS`.
pub fn has_clock_time(line: &str) -> bool {
    let b = line.as_bytes();
    b.windows(8).any(|w| {
        w[2] == b':' && w[5] == b':' && [0, 1, 3, 4, 6, 7].iter().all(|&i| w[i].is_ascii_digit())
    })
}
