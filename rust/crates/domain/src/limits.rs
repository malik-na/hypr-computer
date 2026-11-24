//! The limits the user puts on agents with `hypr-computer allow`, and the block on password managers, which
//! is always on (specs/Limits.tla). Out of the box an agent may do anything on its own screen but use a
//! password manager or start a browser itself. One `Limits` holds for every agent; the guard asks it at every
//! tool call, and the proxy at every request, with the time then.

mod command;
mod describe;
mod names;
mod record;
mod site;

use std::collections::BTreeSet;
use std::fmt;
use std::time::{Duration, SystemTime};

pub use command::{AllowCommand, parse_duration};
pub use names::{BROWSER, is_browser, is_password_manager};
pub use site::{normalize_site, site_matches};

/// What one `hypr-computer allow` asks for. `None`: no limit of that kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Allow {
    /// Only these apps: program file names (`foot`, `chromium`) or [`BROWSER`], in lower case.
    pub apps: Option<BTreeSet<String>>,
    /// Only these sites, as hosts (`example.com`, `127.0.0.2`), in lower case. A site includes its
    /// subdomains.
    pub sites: Option<BTreeSet<String>>,
    /// Look only: every tool but the ones that only read the screen is refused.
    pub read_only: bool,
    /// For how long; `None`: until `hypr-computer allow --off`.
    pub duration: Option<Duration>,
}

impl Allow {
    fn limits_something(&self) -> bool {
        self.apps.is_some() || self.sites.is_some() || self.read_only
    }
}

/// What an agent asks for, as the guard sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action<'a> {
    /// A tool that only reads the screen: screenshot.
    Look,
    /// Input to the app whose window is in front: click, move, type, key, scroll, drag. `app` is that app's
    /// name (the file name of the program launch_app started, or [`BROWSER`]); `None` with no window in
    /// front, or one no launch of the agent's made.
    Input { app: Option<&'a str> },
    /// launch_app of `program`: a file name, or a path, whose file name counts.
    Launch { program: &'a str },
    /// open_url of a page on `host`.
    Open { host: &'a str },
}

/// Why a call is refused. Its `Display` is the text the agent gets, as a failed tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Read-only mode.
    ReadOnly,
    /// An app limit that doesn't name `app`.
    App { app: String },
    /// Input under an app limit with no window of an app in front.
    NoApp,
    /// A site limit that doesn't list `host`.
    Site { host: String },
    /// `name` is a password manager or a password prompt: never, whatever the limits.
    PasswordManager { name: String },
    /// launch_app of a browser, which only open_url starts.
    Browser { program: String },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::ReadOnly => f.write_str(
                "Refused: the user has put agents in read-only mode: you may look at your screen \
                 (screenshot), but not click, type or start anything. Ask the user if you need to do more.",
            ),
            Refusal::App { app } => write!(
                f,
                "Refused: the user hasn't allowed agents to use {app}. Ask the user to allow it \
                 (hypr-computer allow)."
            ),
            Refusal::NoApp => f.write_str(
                "Refused: the user has limited agents to some apps, and no window of one of them is \
                 in front. Ask the user, or bring one of those apps to the front with launch_app.",
            ),
            Refusal::Site { host } => write!(
                f,
                "Refused: the user hasn't allowed agents to open pages on {host}. Ask the user to \
                 allow it (hypr-computer allow --sites)."
            ),
            Refusal::PasswordManager { name } => write!(
                f,
                "Refused: {name} is a password manager or a password prompt, which agents may never \
                 see or use. Ask the user to do this part themselves."
            ),
            Refusal::Browser { program } => write!(
                f,
                "Refused: {program} is a web browser, which agents start only through open_url, so \
                 that it goes through hypr-computer's proxy. Call open_url with the page you want."
            ),
        }
    }
}

/// The limits in force: what the user's last `hypr-computer allow` set, and until when.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    /// What `allow` asked for, without its duration; the default for no limit.
    allow: Allow,
    until: Option<SystemTime>,
}

impl Limits {
    /// No limit: the default, and what `--off` sets.
    pub fn none() -> Self {
        Self::default()
    }

    /// The limits `allow` asks for, set at `now`: with a duration, their time is up at `now + duration`.
    /// A time too far off for the clock never comes.
    pub fn set(allow: &Allow, now: SystemTime) -> Self {
        if !allow.limits_something() {
            return Self::none();
        }
        Self {
            allow: Allow {
                duration: None,
                ..allow.clone()
            },
            until: allow.duration.and_then(|d| now.checked_add(d)),
        }
    }

    /// When their time is up; `None` for limits set without a duration, and for no limit.
    pub fn until(&self) -> Option<SystemTime> {
        self.until
    }

    /// Whether they limit anything at `now`: false for no limit, and once their time is up.
    pub fn in_force(&self, now: SystemTime) -> bool {
        self.allow.limits_something() && self.until.is_none_or(|until| now < until)
    }

    /// What applies at `now`: the limits set, or nothing once their time is up.
    fn in_effect(&self, now: SystemTime) -> Option<&Allow> {
        self.in_force(now).then_some(&self.allow)
    }

    /// Whether `action` may go ahead at `now`; if not, why. The spec's `Admits`.
    pub fn check(&self, action: Action<'_>, now: SystemTime) -> Result<(), Refusal> {
        let nothing = Allow::default();
        let e = self.in_effect(now).unwrap_or(&nothing);
        match action {
            Action::Look => Ok(()),
            Action::Input { app } => {
                if let Some(app) = app.filter(|app| is_password_manager(app)) {
                    return Err(Refusal::PasswordManager {
                        name: app.to_owned(),
                    });
                }
                if e.read_only {
                    return Err(Refusal::ReadOnly);
                }
                match (&e.apps, app) {
                    (None, _) => Ok(()),
                    (Some(_), None) => Err(Refusal::NoApp),
                    (Some(apps), Some(app)) => names(apps, app),
                }
            }
            Action::Launch { program } => {
                let name = file_name(program);
                if is_password_manager(name) {
                    return Err(Refusal::PasswordManager {
                        name: name.to_owned(),
                    });
                }
                if is_browser(program) {
                    return Err(Refusal::Browser {
                        program: name.to_owned(),
                    });
                }
                if e.read_only {
                    return Err(Refusal::ReadOnly);
                }
                e.apps.as_ref().map_or(Ok(()), |apps| names(apps, name))
            }
            Action::Open { host } => {
                if e.read_only {
                    return Err(Refusal::ReadOnly);
                }
                if let Some(apps) = &e.apps {
                    names(apps, BROWSER)?;
                }
                if lists(e.sites.as_ref(), host) {
                    Ok(())
                } else {
                    Err(Refusal::Site {
                        host: host.to_owned(),
                    })
                }
            }
        }
    }

    /// Whether the browser may load a page on `host` at `now`: the proxy's check (`ProxyLets`). Only the
    /// sites limit counts here; open_url's own call is `check`ed as an [`Action::Open`].
    pub fn allows_site(&self, host: &str, now: SystemTime) -> bool {
        lists(self.in_effect(now).and_then(|e| e.sites.as_ref()), host)
    }
}

/// Whether the app limit `apps` names `app`.
fn names(apps: &BTreeSet<String>, app: &str) -> Result<(), Refusal> {
    let app_lower = app.to_lowercase();
    if apps.iter().any(|a| a.to_lowercase() == app_lower) {
        Ok(())
    } else {
        Err(Refusal::App {
            app: app.to_owned(),
        })
    }
}

/// Whether the site limit `sites`, if there is one, lists `host`.
fn lists(sites: Option<&BTreeSet<String>>, host: &str) -> bool {
    sites.is_none_or(|sites| sites.iter().any(|site| site_matches(site, host)))
}

/// The file name of `program`, a file name or a path.
fn file_name(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}
