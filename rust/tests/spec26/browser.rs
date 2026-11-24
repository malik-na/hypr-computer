//! The browser the agent's pages open in: this machine's Chromium, started
//! through a wrapper that stands for the system's browser command. The
//! wrapper notes each launch (its pid, arguments and environment), marks the
//! browser's environment so its processes can be found later, and can add
//! arguments of its own, as a distribution's launcher does.

use std::fs;
use std::path::{Path, PathBuf};

use super::Env;

/// This machine's Chromium-family browser: `$HC26_BROWSER` if set (`none`
/// for no browser), otherwise the first of the usual names on PATH.
pub fn find() -> Option<PathBuf> {
    if let Some(given) = std::env::var_os("HC26_BROWSER") {
        return (given != "none").then(|| PathBuf::from(given));
    }
    let path = std::env::var_os("PATH")?;
    let names = [
        "chromium",
        "chromium-browser",
        "google-chrome-stable",
        "google-chrome",
    ];
    names.iter().find_map(|name| {
        std::env::split_paths(&path)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

/// The wrapper the daemon is told to start as the browser.
pub struct Wrapper {
    path: PathBuf,
    dir: PathBuf,
    mark: String,
}

impl Wrapper {
    /// A wrapper for `real`, adding `extra` arguments before the daemon's.
    pub fn new(env: &Env, real: &Path, extra: &[String]) -> Self {
        let dir = env.path("browser");
        fs::create_dir_all(&dir).unwrap();
        let mark = format!(
            "{}-{}",
            env.home().parent().unwrap().display(),
            std::process::id()
        )
        .replace('/', "");
        let quoted: Vec<String> = extra.iter().map(|a| quote(a)).collect();
        // Named like the browser, as the program a system would start.
        let path = env.script_at(
            &env.path("bin").join("chromium"),
            &format!(
                "echo $$ >> {pids}\n\
                 {{ echo '--- launch'; printf '%s\\n' \"$@\"; }} >> {args}\n\
                 env > {dir}/env-$$\n\
                 export HC26_MARK={mark}\n\
                 exec {real} {extra} \"$@\"",
                pids = quote(&dir.join("pids").to_string_lossy()),
                args = quote(&dir.join("args").to_string_lossy()),
                dir = quote(&dir.to_string_lossy()),
                mark = quote(&mark),
                real = quote(&real.to_string_lossy()),
                extra = quoted.join(" "),
            ),
        );
        Wrapper { path, dir, mark }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The value of `HC26_MARK` in every process the browser starts.
    pub fn mark(&self) -> &str {
        &self.mark
    }

    /// The pids of each time the browser was started.
    pub fn launches(&self) -> Vec<u32> {
        fs::read_to_string(self.dir.join("pids"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect()
    }

    /// The arguments of every launch, all together.
    pub fn args(&self) -> Vec<String> {
        fs::read_to_string(self.dir.join("args"))
            .unwrap_or_default()
            .lines()
            .filter(|l| *l != "--- launch")
            .map(str::to_owned)
            .collect()
    }

    /// The environment launch `pid` was started with.
    pub fn env_of(&self, pid: u32) -> Vec<(String, String)> {
        fs::read_to_string(self.dir.join(format!("env-{pid}")))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    }

    /// The profile directories the browser was started with: each
    /// `--user-data-dir`, or else the home it was given.
    pub fn profiles(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self
            .args()
            .iter()
            .filter_map(|a| a.strip_prefix("--user-data-dir="))
            .map(PathBuf::from)
            .collect();
        if dirs.is_empty() {
            for pid in self.launches() {
                if let Some((_, home)) = self.env_of(pid).into_iter().find(|(k, _)| k == "HOME") {
                    dirs.push(PathBuf::from(home));
                }
            }
        }
        dirs.sort();
        dirs.dedup();
        dirs
    }
}

/// `s` quoted for sh.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
