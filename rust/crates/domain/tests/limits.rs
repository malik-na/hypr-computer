//! The limits the user puts on agents, tested at their seam: `domain::limits`, against specs/Limits.tla.
//! Written from the spec (#31, #32 and specs/Limits.tla) by an independent test agent.
//!
//! # The API these tests fix
//!
//! `rust/crates/domain/src/limits.rs`, declared `pub mod limits;` in `lib.rs`. Pure: the time comes in as an
//! argument, and nothing here reads a clock, a file or the network.
//!
//! ```ignore
//! use std::collections::BTreeSet;
//! use std::time::{Duration, SystemTime};
//!
//! /// The name an app limit gives the agent's browser, the one open_url starts: `hypr-computer allow browser`.
//! pub const BROWSER: &str = "browser";
//!
//! /// What one `hypr-computer allow` asks for. `None`: no limit of that kind.
//! #[derive(Clone, Debug, Default, PartialEq, Eq)]
//! pub struct Allow {
//!     /// Only these apps: program file names (`foot`, `chromium`) or `browser`, in lower case.
//!     pub apps: Option<BTreeSet<String>>,
//!     /// Only these sites, as hosts (`example.com`, `127.0.0.2`), in lower case. A site includes its subdomains.
//!     pub sites: Option<BTreeSet<String>>,
//!     /// Look only: every tool but the ones that only read the screen is refused.
//!     pub read_only: bool,
//!     /// For how long; `None`: until `hypr-computer allow --off`.
//!     pub duration: Option<Duration>,
//! }
//!
//! /// `hypr-computer allow <args>`: what the user asked for.
//! #[derive(Clone, Debug, PartialEq, Eq)]
//! pub enum AllowCommand {
//!     /// Replace the limits with these.
//!     Set(Allow),
//!     /// `--list`, or no arguments: show the limits in force.
//!     List,
//!     /// `--off`: no limits.
//!     Off,
//! }
//!
//! impl AllowCommand {
//!     /// Parses the arguments after `allow`. `Err` says, in words for the user, what is wrong with them; the
//!     /// command prints it with its usage and exits 2, and the limits don't change.
//!     pub fn parse(args: &[&str]) -> Result<AllowCommand, String>;
//! }
//!
//! /// `30m`, `2h`, `45s`, `1h30m`, `1h30m15s`: hours, minutes and seconds in that order, each at most once,
//! /// digits only. `None` for anything else, for zero, and for a time too long to add to a clock.
//! pub fn parse_duration(text: &str) -> Option<Duration>;
//!
//! /// A site as `--sites` takes it, reduced to its host: lower case, without a scheme, user, port, path,
//! /// query, fragment, trailing dot or IPv6 brackets. `None` if no host is left, or it has a character a host
//! /// can't have (a space, a `*`, a `/` left over).
//! pub fn normalize_site(text: &str) -> Option<String>;
//!
//! /// Whether a page on `host` is on `site` (normalized): the site itself or a subdomain of it. An IP address
//! /// matches only itself. `host` is compared as `normalize_site` would reduce it.
//! pub fn site_matches(site: &str, host: &str) -> bool;
//!
//! /// Whether `name`, a program's file name or a window's app id, is a password manager or a password prompt.
//! /// Case-insensitive; true when it contains any of these: `1password`, `bitwarden`, `keepass`, `lastpass`,
//! /// `enpass`, `dashlane`, `proton-pass`, `protonpass`, `proton pass`, `seahorse`, `org.gnome.world.secrets`,
//! /// `gnome-secrets`, `passwordsafe`, `kwallet`, `pinentry`, `polkit`, `gcr-prompter`, `askpass`.
//! pub fn is_password_manager(name: &str) -> bool;
//!
//! /// Whether `program` (a file name, or a path, whose file name counts) is a web browser, which only open_url
//! /// may start, pointed at the proxy. Case-insensitive, the whole file name: `browser`, `chromium`,
//! /// `chromium-browser`, `chrome`, `google-chrome`, `google-chrome-stable`, `google-chrome-beta`,
//! /// `google-chrome-unstable`, `brave`, `brave-browser`, `microsoft-edge`, `microsoft-edge-stable`, `vivaldi`,
//! /// `vivaldi-stable`, `opera`, `thorium-browser`, `firefox`, `firefox-esr`, `librewolf`, `zen-browser`,
//! /// `floorp`, `epiphany`, `falkon`, `qutebrowser`.
//! pub fn is_browser(program: &str) -> bool;
//!
//! /// What an agent asks for, as the guard sees it.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Action<'a> {
//!     /// A tool that only reads the screen: screenshot.
//!     Look,
//!     /// Input to the app whose window is in front: click, move, type, key, scroll, drag. `app` is that app's
//!     /// name (the file name of the program launch_app started, or `BROWSER`); `None` with no window in front,
//!     /// or one no launch of the agent's made.
//!     Input { app: Option<&'a str> },
//!     /// launch_app of `program`: a file name, or a path, whose file name counts.
//!     Launch { program: &'a str },
//!     /// open_url of a page on `host`.
//!     Open { host: &'a str },
//! }
//!
//! /// Why a call is refused. Its `Display` is the text the agent gets, as a failed tool call. Every one but
//! /// `Browser` tells the agent to ask the user ("Ask the user"), and names what was refused.
//! #[derive(Clone, Debug, PartialEq, Eq)]
//! pub enum Refusal {
//!     /// Read-only mode: says "read-only".
//!     ReadOnly,
//!     /// An app limit that doesn't name `app`.
//!     App { app: String },
//!     /// Input under an app limit with no window of an app in front.
//!     NoApp,
//!     /// A site limit that doesn't list `host`.
//!     Site { host: String },
//!     /// `name` is a password manager or a password prompt: never, whatever the limits. Says "password".
//!     PasswordManager { name: String },
//!     /// launch_app of a browser: says to use open_url.
//!     Browser { program: String },
//! }
//! impl std::fmt::Display for Refusal { ... }
//!
//! /// The limits in force: what the user's last `hypr-computer allow` set, and until when.
//! #[derive(Clone, Debug, Default, PartialEq, Eq)]     // Default: no limit
//! pub struct Limits { /* private */ }
//!
//! impl Limits {
//!     /// No limit: the default, and what `--off` sets.
//!     pub fn none() -> Self;
//!     /// The limits `allow` asks for, set at `now`: with a duration, their time is up at `now + duration`.
//!     pub fn set(allow: &Allow, now: SystemTime) -> Self;
//!     /// When their time is up; `None` for limits set without a duration, and for no limit.
//!     pub fn until(&self) -> Option<SystemTime>;
//!     /// Whether they limit anything at `now`: false for no limit, and once their time is up (`now >= until`).
//!     pub fn in_force(&self, now: SystemTime) -> bool;
//!     /// Whether `action` may go ahead at `now`; if not, why. Specs/Limits.tla's `Admits`.
//!     pub fn check(&self, action: Action<'_>, now: SystemTime) -> Result<(), Refusal>;
//!     /// Whether the browser may load a page on `host` at `now`: the proxy's check (`ProxyLets`). Only the
//!     /// sites limit counts here; open_url's own call is `check`ed as an `Open`.
//!     pub fn allows_site(&self, host: &str, now: SystemTime) -> bool;
//!     /// In words for the user (`hypr-computer allow --list`): "No limits." when none is in force at `now`;
//!     /// otherwise each app, each site, "read-only" if so, and "until" with the time if they have one.
//!     pub fn describe(&self, now: SystemTime) -> String;
//! }
//! ```
//!
//! `check`, with `E` the limits in force at `now` (no limit once their time is up), in this order:
//!
//! | Action | Refused with | When |
//! | --- | --- | --- |
//! | any | | never for `Look` |
//! | `Launch`, `Input` | `PasswordManager` | the program or app is a password manager, whatever `E` |
//! | `Launch` | `Browser` | the program is a browser, whatever `E` |
//! | `Input`, `Launch`, `Open` | `ReadOnly` | `E` is read-only |
//! | `Input` | `NoApp` | `E` limits apps and `app` is `None` |
//! | `Input`, `Launch` | `App` | `E` limits apps and doesn't name the app (its file name, case-insensitive) |
//! | `Open` | `App { app: "browser" }` | `E` limits apps and doesn't name `browser` |
//! | `Open` | `Site` | `E` limits sites and `host` is on none of them |
//!
//! `AllowCommand::parse`: apps are the arguments that aren't options, each a comma-separated list; empty
//! pieces are dropped, names are lower-cased and duplicates merged. `--sites LIST` (or `--sites=LIST`, and it
//! may be given again: the lists are joined), `--read-only`, `--for DURATION` (or `--for=DURATION`, once).
//! Errors: nothing to allow (no app, no site, no read-only), an unknown option or one missing its value, a
//! bad duration, an empty or bad site, an app name with a `/`, `--read-only` with apps or sites (read-only
//! already allows only looking), `--list` or `--off` with anything else. Each error names the offending
//! argument where there is one.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use domain::limits::{
    Action, Allow, AllowCommand, BROWSER, Limits, Refusal, is_browser, is_password_manager,
    normalize_site, parse_duration, site_matches,
};

/// A fixed moment, so that nothing depends on the clock of the machine running the tests.
fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn at(secs: u64) -> SystemTime {
    t0() + Duration::from_secs(secs)
}

fn set(names: &[&str]) -> Option<BTreeSet<String>> {
    Some(names.iter().map(|s| (*s).to_owned()).collect())
}

fn apps(names: &[&str]) -> Allow {
    Allow {
        apps: set(names),
        ..Allow::default()
    }
}

fn sites(names: &[&str]) -> Allow {
    Allow {
        sites: set(names),
        ..Allow::default()
    }
}

fn read_only() -> Allow {
    Allow {
        read_only: true,
        ..Allow::default()
    }
}

fn parse(args: &[&str]) -> Result<AllowCommand, String> {
    AllowCommand::parse(args)
}

#[track_caller]
fn parsed(args: &[&str]) -> Allow {
    let got = parse(args);
    assert!(
        matches!(got, Ok(AllowCommand::Set(_))),
        "`allow {}` should set limits, got {got:?}",
        args.join(" ")
    );
    match got {
        Ok(AllowCommand::Set(allow)) => allow,
        _ => Allow::default(),
    }
}

#[track_caller]
fn ok(limits: &Limits, action: Action<'_>, now: SystemTime) {
    assert_eq!(
        limits.check(action, now),
        Ok(()),
        "{action:?} under {limits:?}"
    );
}

#[track_caller]
fn refused(limits: &Limits, action: Action<'_>, now: SystemTime) -> Refusal {
    let got = limits.check(action, now);
    assert!(got.is_err(), "{action:?} must be refused under {limits:?}");
    got.err().unwrap_or(Refusal::NoApp)
}

/// Every refusal of a limit tells the agent to ask the user.
#[track_caller]
fn asks_the_user(why: &Refusal) -> String {
    let text = why.to_string();
    assert!(
        text.to_lowercase().contains("ask the user"),
        "{why:?} must tell the agent to ask the user; it says {text:?}"
    );
    text
}

const EVERY_KIND: [Action<'static>; 6] = [
    Action::Look,
    Action::Input { app: Some("notes") },
    Action::Input { app: None },
    Action::Input { app: Some(BROWSER) },
    Action::Launch { program: "notes" },
    Action::Open {
        host: "example.com",
    },
];

// ---- No limit: the default.

#[test]
fn with_no_limit_everything_is_allowed_but_a_password_manager_and_launching_a_browser() {
    for limits in [Limits::none(), Limits::default()] {
        assert!(!limits.in_force(t0()));
        assert_eq!(limits.until(), None);
        for action in EVERY_KIND {
            ok(&limits, action, t0());
        }
        ok(
            &limits,
            Action::Launch {
                program: "/usr/bin/foot",
            },
            t0(),
        );
        ok(&limits, Action::Open { host: "127.0.0.3" }, t0());
        assert!(limits.allows_site("anything.example", t0()));
        assert!(matches!(
            refused(
                &limits,
                Action::Launch {
                    program: "keepassxc"
                },
                t0()
            ),
            Refusal::PasswordManager { .. }
        ));
        assert!(matches!(
            refused(
                &limits,
                Action::Input {
                    app: Some("1password")
                },
                t0()
            ),
            Refusal::PasswordManager { .. }
        ));
        assert!(matches!(
            refused(
                &limits,
                Action::Launch {
                    program: "chromium"
                },
                t0()
            ),
            Refusal::Browser { .. }
        ));
    }
    assert_eq!(Limits::none(), Limits::default());
    assert_eq!(Limits::set(&Allow::default(), t0()), Limits::none());
}

// ---- Allowed apps.

#[test]
fn an_app_limit_allows_only_the_apps_it_names() {
    let limits = Limits::set(&apps(&["notes", "foot"]), t0());
    assert!(limits.in_force(t0()));
    ok(&limits, Action::Look, t0());
    ok(&limits, Action::Launch { program: "notes" }, t0());
    ok(
        &limits,
        Action::Launch {
            program: "/usr/bin/foot",
        },
        t0(),
    );
    ok(&limits, Action::Launch { program: "FOOT" }, t0());
    ok(&limits, Action::Input { app: Some("notes") }, t0());
    ok(&limits, Action::Input { app: Some("foot") }, t0());

    let why = refused(
        &limits,
        Action::Launch {
            program: "/opt/paint",
        },
        t0(),
    );
    assert_eq!(
        why,
        Refusal::App {
            app: "paint".to_owned()
        }
    );
    assert!(asks_the_user(&why).contains("paint"));

    let why = refused(&limits, Action::Input { app: Some("paint") }, t0());
    assert_eq!(
        why,
        Refusal::App {
            app: "paint".to_owned()
        }
    );

    // Nothing in front: there is no app the input may go to.
    let why = refused(&limits, Action::Input { app: None }, t0());
    assert_eq!(why, Refusal::NoApp);
    asks_the_user(&why);
}

#[test]
fn an_app_limit_without_browser_refuses_open_url_and_input_to_the_browser() {
    let limits = Limits::set(&apps(&["notes"]), t0());
    let why = refused(
        &limits,
        Action::Open {
            host: "example.com",
        },
        t0(),
    );
    assert_eq!(
        why,
        Refusal::App {
            app: BROWSER.to_owned()
        }
    );
    assert!(asks_the_user(&why).contains(BROWSER));
    let why = refused(&limits, Action::Input { app: Some(BROWSER) }, t0());
    assert_eq!(
        why,
        Refusal::App {
            app: BROWSER.to_owned()
        }
    );
    // The app limit is about apps: the proxy lets the browser load any site.
    assert!(limits.allows_site("example.com", t0()));

    let limits = Limits::set(&apps(&["notes", BROWSER]), t0());
    ok(
        &limits,
        Action::Open {
            host: "example.com",
        },
        t0(),
    );
    ok(&limits, Action::Input { app: Some(BROWSER) }, t0());
}

#[test]
fn naming_a_password_manager_or_a_browser_in_a_limit_doesnt_let_it_be_launched() {
    let limits = Limits::set(&apps(&["keepassxc", "chromium", "pinentry-gtk"]), t0());
    for program in ["keepassxc", "/usr/bin/pinentry-gtk"] {
        let why = refused(&limits, Action::Launch { program }, t0());
        assert!(matches!(why, Refusal::PasswordManager { .. }), "{why:?}");
        assert!(asks_the_user(&why).to_lowercase().contains("password"));
    }
    let why = refused(
        &limits,
        Action::Input {
            app: Some("keepassxc"),
        },
        t0(),
    );
    assert!(matches!(why, Refusal::PasswordManager { .. }), "{why:?}");
    let why = refused(
        &limits,
        Action::Launch {
            program: "chromium",
        },
        t0(),
    );
    assert_eq!(
        why,
        Refusal::Browser {
            program: "chromium".to_owned()
        }
    );
    assert!(why.to_string().contains("open_url"), "{why}");
}

// ---- Read-only.

#[test]
fn read_only_allows_only_looking() {
    let limits = Limits::set(&read_only(), t0());
    assert!(limits.in_force(t0()));
    ok(&limits, Action::Look, t0());
    for action in [
        Action::Input { app: Some("notes") },
        Action::Input { app: None },
        Action::Input { app: Some(BROWSER) },
        Action::Launch { program: "notes" },
        Action::Open {
            host: "example.com",
        },
    ] {
        let why = refused(&limits, action, t0());
        assert_eq!(why, Refusal::ReadOnly, "{action:?}");
        assert!(asks_the_user(&why).to_lowercase().contains("read-only"));
    }
}

// ---- Allowed sites.

#[test]
fn a_site_limit_lets_the_browser_load_only_listed_sites_and_their_subdomains() {
    let limits = Limits::set(&sites(&["example.com", "127.0.0.2"]), t0());
    assert!(limits.in_force(t0()));
    for host in [
        "example.com",
        "www.example.com",
        "a.b.example.com",
        "EXAMPLE.com",
        "example.com.",
        "127.0.0.2",
    ] {
        assert!(limits.allows_site(host, t0()), "{host} must be allowed");
        ok(&limits, Action::Open { host }, t0());
    }
    for host in [
        "example.org",
        "notexample.com",
        "example.com.evil.net",
        "com",
        "127.0.0.3",
        "0.127.0.0.2",
        "localhost",
    ] {
        assert!(!limits.allows_site(host, t0()), "{host} must be refused");
        let why = refused(&limits, Action::Open { host }, t0());
        assert_eq!(
            why,
            Refusal::Site {
                host: host.to_owned()
            }
        );
        assert!(asks_the_user(&why).contains(host));
    }
    // A site limit is about sites: apps, and input to the browser, are fine.
    ok(&limits, Action::Launch { program: "notes" }, t0());
    ok(&limits, Action::Input { app: Some(BROWSER) }, t0());
    ok(&limits, Action::Input { app: None }, t0());
}

#[test]
fn an_app_limit_and_a_site_limit_together_need_both() {
    let allow = Allow {
        apps: set(&["notes", BROWSER]),
        sites: set(&["example.com"]),
        ..Allow::default()
    };
    let limits = Limits::set(&allow, t0());
    ok(
        &limits,
        Action::Open {
            host: "example.com",
        },
        t0(),
    );
    assert_eq!(
        refused(
            &limits,
            Action::Open {
                host: "example.org"
            },
            t0()
        ),
        Refusal::Site {
            host: "example.org".to_owned()
        }
    );
    assert_eq!(
        refused(&limits, Action::Launch { program: "paint" }, t0()),
        Refusal::App {
            app: "paint".to_owned()
        }
    );

    let allow = Allow {
        apps: set(&["notes"]),
        sites: set(&["example.com"]),
        ..Allow::default()
    };
    let limits = Limits::set(&allow, t0());
    assert_eq!(
        refused(
            &limits,
            Action::Open {
                host: "example.com"
            },
            t0()
        ),
        Refusal::App {
            app: BROWSER.to_owned()
        }
    );
}

#[test]
fn sites_are_matched_by_host_only() {
    assert!(site_matches("example.com", "example.com"));
    assert!(site_matches("example.com", "deep.sub.example.com"));
    assert!(site_matches("example.com", "Sub.Example.COM."));
    assert!(!site_matches("example.com", "badexample.com"));
    assert!(!site_matches("sub.example.com", "example.com"));
    assert!(site_matches("127.0.0.2", "127.0.0.2"));
    assert!(!site_matches("0.0.2", "127.0.0.2"));
    assert!(!site_matches("127.0.0.2", "127.0.0.20"));
    assert!(site_matches("::1", "[::1]"));
    assert!(site_matches("localhost", "localhost"));
}

#[test]
fn a_site_is_reduced_to_its_host() {
    for (given, host) in [
        ("example.com", "example.com"),
        ("Example.COM", "example.com"),
        ("https://example.com", "example.com"),
        (
            "http://www.example.com:8080/path?q=1#top",
            "www.example.com",
        ),
        ("example.com/", "example.com"),
        ("example.com.", "example.com"),
        ("https://user:pw@example.com/", "example.com"),
        ("127.0.0.2", "127.0.0.2"),
        ("127.0.0.2:8000", "127.0.0.2"),
        ("[::1]:8080", "::1"),
        ("localhost", "localhost"),
    ] {
        assert_eq!(normalize_site(given).as_deref(), Some(host), "{given:?}");
    }
    for bad in [
        "",
        " ",
        "https://",
        "*.example.com",
        "exa mple.com",
        "http:///x",
        ":8080",
    ] {
        assert_eq!(normalize_site(bad), None, "{bad:?} isn't a site");
    }
}

// ---- Expiry.

#[test]
fn an_expired_limit_stops_applying() {
    for allow in [apps(&["notes"]), sites(&["example.com"]), read_only()] {
        let allow = Allow {
            duration: Some(Duration::from_secs(30 * 60)),
            ..allow
        };
        let limits = Limits::set(&allow, at(100));
        assert_eq!(limits.until(), Some(at(100 + 30 * 60)));
        assert!(limits.in_force(at(100)));
        assert!(limits.in_force(at(100 + 30 * 60 - 1)));
        // In force: something is refused.
        let before = [
            limits.check(Action::Launch { program: "paint" }, at(101)),
            limits.check(
                Action::Open {
                    host: "example.org",
                },
                at(101),
            ),
        ];
        assert!(before.iter().any(Result::is_err), "{allow:?}: {before:?}");
        // Once the time is up, nothing is.
        for now in [at(100 + 30 * 60), at(100 + 30 * 60 + 1), at(10_000_000)] {
            assert!(!limits.in_force(now));
            for action in EVERY_KIND {
                ok(&limits, action, now);
            }
            ok(&limits, Action::Launch { program: "paint" }, now);
            ok(
                &limits,
                Action::Open {
                    host: "example.org",
                },
                now,
            );
            assert!(limits.allows_site("example.org", now));
            assert!(
                limits.describe(now).contains("No limits"),
                "{}",
                limits.describe(now)
            );
        }
        // ... but a password manager is still never launched.
        assert!(matches!(
            refused(
                &limits,
                Action::Launch {
                    program: "bitwarden"
                },
                at(10_000_000)
            ),
            Refusal::PasswordManager { .. }
        ));
    }
}

#[test]
fn a_limit_without_a_duration_never_expires() {
    let limits = Limits::set(&apps(&["notes"]), t0());
    assert_eq!(limits.until(), None);
    let much_later = t0() + Duration::from_secs(100 * 365 * 24 * 3600);
    assert!(limits.in_force(much_later));
    assert!(
        limits
            .check(Action::Launch { program: "paint" }, much_later)
            .is_err()
    );
}

#[test]
fn durations_are_hours_minutes_and_seconds() {
    for (text, secs) in [
        ("30m", 1800),
        ("2h", 7200),
        ("45s", 45),
        ("1h30m", 5400),
        ("1h30m15s", 5415),
        ("90m", 5400),
        ("1m30s", 90),
        ("0h5m", 300),
    ] {
        assert_eq!(
            parse_duration(text),
            Some(Duration::from_secs(secs)),
            "{text:?}"
        );
    }
    for bad in [
        "",
        "0m",
        "0s",
        "0h0m0s",
        "30",
        "m",
        "h30m",
        "30m1h",
        "1.5h",
        "-5m",
        "+5m",
        "5 m",
        "5M",
        "1d",
        "soon",
        "99999999999999999999999h",
        "1h1h",
    ] {
        assert_eq!(parse_duration(bad), None, "{bad:?} isn't a duration");
    }
}

// ---- What the user types.

#[test]
fn the_allow_command_line_is_parsed() {
    assert_eq!(parse(&[]), Ok(AllowCommand::List));
    assert_eq!(parse(&["--list"]), Ok(AllowCommand::List));
    assert_eq!(parse(&["--off"]), Ok(AllowCommand::Off));

    assert_eq!(parsed(&["notes"]), apps(&["notes"]));
    assert_eq!(
        parsed(&["Notes,paint", "foot"]),
        apps(&["notes", "paint", "foot"])
    );
    assert_eq!(
        parsed(&["notes,,notes,", "browser"]),
        apps(&["notes", "browser"])
    );
    assert_eq!(
        parsed(&["notes", "--for", "30m"]),
        Allow {
            duration: Some(Duration::from_secs(1800)),
            ..apps(&["notes"])
        }
    );
    assert_eq!(
        parsed(&["--for=1h30m", "notes"]),
        Allow {
            duration: Some(Duration::from_secs(5400)),
            ..apps(&["notes"])
        }
    );
    assert_eq!(
        parsed(&[
            "--sites",
            "Example.com,https://docs.rs/serde",
            "--sites=127.0.0.2"
        ]),
        sites(&["example.com", "docs.rs", "127.0.0.2"])
    );
    assert_eq!(
        parsed(&["notes", "browser", "--sites", "example.com", "--for", "10m"]),
        Allow {
            apps: set(&["notes", "browser"]),
            sites: set(&["example.com"]),
            read_only: false,
            duration: Some(Duration::from_secs(600)),
        }
    );
    assert_eq!(parsed(&["--read-only"]), read_only());
    assert_eq!(
        parsed(&["--read-only", "--for", "45s"]),
        Allow {
            duration: Some(Duration::from_secs(45)),
            ..read_only()
        }
    );
}

#[test]
fn a_bad_allow_command_line_says_what_is_wrong() {
    for (args, names) in [
        (&["--for", "30m"][..], ""),
        (&["notes", "--for"], "--for"),
        (&["notes", "--for", "0m"], "0m"),
        (&["notes", "--for", "soon"], "soon"),
        (&["notes", "--for", "5m", "--for", "6m"], "--for"),
        (&["--sites"], "--sites"),
        (&["--sites", ""], "--sites"),
        (&["--sites", "*.example.com"], "*.example.com"),
        (&["--bogus"], "--bogus"),
        (&["notes", "-x"], "-x"),
        (&["/usr/bin/foot"], "/usr/bin/foot"),
        (&["--read-only", "notes"], "--read-only"),
        (&["--read-only", "--sites", "example.com"], "--read-only"),
        (&["--list", "notes"], ""),
        (&["--off", "--for", "5m"], ""),
        (&["--off", "--list"], ""),
        (&[",", ","], ""),
    ] {
        match parse(args) {
            Ok(got) => panic!("`allow {}` must be an error, got {got:?}", args.join(" ")),
            Err(why) => {
                assert!(
                    !why.trim().is_empty(),
                    "`allow {}`: an empty error",
                    args.join(" ")
                );
                assert!(
                    why.contains(names),
                    "`allow {}`: the error should name {names:?}; it says {why:?}",
                    args.join(" ")
                );
            }
        }
    }
}

// ---- The lists.

#[test]
fn password_managers_and_prompts_are_known_by_program_or_app_id() {
    for name in [
        "keepassxc",
        "org.keepassxc.KeePassXC",
        "KeePass2",
        "1password",
        "1Password",
        "bitwarden",
        "Bitwarden",
        "proton-pass",
        "Proton Pass",
        "org.gnome.World.Secrets",
        "gnome-secrets",
        "org.gnome.seahorse.Application",
        "seahorse",
        "org.gnome.PasswordSafe",
        "kwalletmanager5",
        "pinentry",
        "pinentry-gtk-2",
        "pinentry-gnome3",
        "pinentry-qt",
        "polkit-gnome-authentication-agent-1",
        "hyprpolkitagent",
        "gcr-prompter",
        "ssh-askpass",
        "ksshaskpass",
        "lastpass",
        "enpass",
        "dashlane",
    ] {
        assert!(
            is_password_manager(name),
            "{name} is a password manager or prompt"
        );
    }
    for name in [
        "foot",
        "notes",
        "paint",
        "chromium",
        "firefox",
        "hc-test-app",
        "org.gnome.Nautilus",
        "kitty",
        "passport",
        "",
    ] {
        assert!(
            !is_password_manager(name),
            "{name} isn't a password manager"
        );
    }
}

#[test]
fn browsers_are_known_by_their_program() {
    for program in [
        "browser",
        "chromium",
        "/usr/bin/chromium",
        "Chromium",
        "chromium-browser",
        "chrome",
        "google-chrome",
        "google-chrome-stable",
        "/opt/google/chrome/google-chrome",
        "brave",
        "brave-browser",
        "microsoft-edge",
        "microsoft-edge-stable",
        "vivaldi",
        "vivaldi-stable",
        "opera",
        "firefox",
        "firefox-esr",
        "librewolf",
        "zen-browser",
        "floorp",
        "epiphany",
        "falkon",
        "qutebrowser",
    ] {
        assert!(is_browser(program), "{program} is a browser");
    }
    for program in [
        "foot",
        "notes",
        "chromium-notes",
        "/usr/bin/chromium/foot",
        "hc-test-app",
        "",
    ] {
        assert!(!is_browser(program), "{program} isn't a browser");
    }
}

// ---- What the user is told.

#[test]
fn the_limits_are_described_for_the_user() {
    assert!(Limits::none().describe(t0()).contains("No limits"));

    let allow = Allow {
        apps: set(&["notes", BROWSER]),
        sites: set(&["example.com"]),
        read_only: false,
        duration: Some(Duration::from_secs(600)),
    };
    let text = Limits::set(&allow, t0()).describe(at(1));
    for word in ["notes", BROWSER, "example.com", "until"] {
        assert!(
            text.contains(word),
            "the description should mention {word:?}: {text:?}"
        );
    }
    assert!(!text.contains("No limits"), "{text:?}");

    let text = Limits::set(&read_only(), t0()).describe(at(1));
    assert!(text.contains("read-only"), "{text:?}");
    assert!(
        !text.contains("until"),
        "a limit without a duration has no end: {text:?}"
    );
}

#[test]
fn each_refusal_says_what_was_refused() {
    let cases = [
        (Refusal::ReadOnly, "read-only"),
        (
            Refusal::App {
                app: "paint".to_owned(),
            },
            "paint",
        ),
        (Refusal::NoApp, ""),
        (
            Refusal::Site {
                host: "example.org".to_owned(),
            },
            "example.org",
        ),
        (
            Refusal::PasswordManager {
                name: "keepassxc".to_owned(),
            },
            "password",
        ),
    ];
    for (why, word) in cases {
        let text = asks_the_user(&why);
        assert!(text.to_lowercase().contains(word), "{why:?}: {text:?}");
    }
    let text = Refusal::Browser {
        program: "chromium".to_owned(),
    }
    .to_string();
    assert!(
        text.contains("open_url") && text.contains("chromium"),
        "{text:?}"
    );
}

#[test]
fn a_new_limit_replaces_the_one_before() {
    // `allow` replaces; `check` goes by the limits it is given, so the caller asks the newest at every call.
    let first = Limits::set(&apps(&["notes"]), t0());
    let second = Limits::set(&sites(&["example.com"]), at(10));
    assert!(
        first
            .check(Action::Launch { program: "paint" }, at(11))
            .is_err()
    );
    ok(&second, Action::Launch { program: "paint" }, at(11));
    assert!(
        second
            .check(
                Action::Open {
                    host: "example.org"
                },
                at(11)
            )
            .is_err()
    );
    assert!(
        first
            .check(
                Action::Open {
                    host: "example.com"
                },
                at(11)
            )
            .is_err()
    );
    ok(
        &second,
        Action::Open {
            host: "www.example.com",
        },
        at(11),
    );
}
