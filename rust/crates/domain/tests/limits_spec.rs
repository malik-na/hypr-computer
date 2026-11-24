//! `domain::limits` against the properties of specs/Limits.tla, over random runs: the user sets limits (any
//! mix of apps, sites, read-only and a duration), turns them off, the clock runs, and agents ask for any
//! action at any time. The API is described at the top of tests/limits.rs.
//!
//! The caller plays the daemon the way the spec's correct design does: it asks the newest limits at every call
//! (`Read`), with the time of that call. What the spec says the limits allow (`Permits(InEffect(limits))`) is
//! written out again here, from the spec, as the oracle.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use domain::limits::{Action, Allow, BROWSER, Limits, Refusal, is_browser, is_password_manager};
use proptest::prelude::*;

const APPS: [&str; 5] = ["notes", "paint", BROWSER, "keepassxc", "chromium"];
const SITES: [&str; 3] = ["example.com", "docs.example.com", "127.0.0.2"];
const HOSTS: [&str; 5] = [
    "example.com",
    "www.example.com",
    "docs.example.com",
    "127.0.0.2",
    "example.org",
];

/// The spec's `Acts`, with the names above.
#[derive(Clone, Copy, Debug)]
enum Act {
    Look,
    Input(Option<usize>),
    Launch(usize),
    Open(usize),
}

impl Act {
    fn action(self) -> Action<'static> {
        match self {
            Act::Look => Action::Look,
            Act::Input(app) => Action::Input {
                app: app.map(|i| APPS[i]),
            },
            Act::Launch(i) => Action::Launch { program: APPS[i] },
            Act::Open(i) => Action::Open { host: HOSTS[i] },
        }
    }
}

#[derive(Clone, Debug)]
enum Step {
    /// `hypr-computer allow`.
    Set(Allow),
    /// `hypr-computer allow --off`.
    Off,
    /// The clock runs.
    Tick(u64),
    /// An agent asks.
    Call(Act),
    /// The browser asks the proxy for a page on `HOSTS[i]`.
    Fetch(usize),
}

fn subset(pool: &'static [&'static str]) -> impl Strategy<Value = Option<BTreeSet<String>>> {
    prop::option::of(prop::collection::btree_set(0..pool.len(), 1..=pool.len())).prop_map(
        move |picked| picked.map(|is| is.into_iter().map(|i| pool[i].to_owned()).collect()),
    )
}

fn allow() -> impl Strategy<Value = Allow> {
    (
        subset(&APPS),
        subset(&SITES),
        any::<bool>(),
        prop::option::of(1u64..300),
    )
        .prop_map(|(apps, sites, read_only, secs)| Allow {
            apps,
            sites,
            read_only,
            duration: secs.map(Duration::from_secs),
        })
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        Just(Act::Look),
        prop::option::of(0..APPS.len()).prop_map(Act::Input),
        (0..APPS.len()).prop_map(Act::Launch),
        (0..HOSTS.len()).prop_map(Act::Open),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        2 => allow().prop_map(Step::Set),
        1 => Just(Step::Off),
        2 => (0u64..200).prop_map(Step::Tick),
        6 => act().prop_map(Step::Call),
        2 => (0..HOSTS.len()).prop_map(Step::Fetch),
    ]
}

/// What the user's last `allow` set, and when: the spec's `limits`.
#[derive(Clone, Debug, Default)]
struct Intent {
    allow: Allow,
    at: Option<SystemTime>,
}

impl Intent {
    /// The spec's `InForce`: no limit is never in force; one with a duration stops at `at + duration`.
    fn in_force(&self, now: SystemTime) -> bool {
        let Some(at) = self.at else { return false };
        let limits_something =
            self.allow.apps.is_some() || self.allow.sites.is_some() || self.allow.read_only;
        limits_something && self.allow.duration.is_none_or(|d| now < at + d)
    }
}

fn site_ok(sites: &Option<BTreeSet<String>>, host: &str) -> bool {
    let host = host.to_lowercase();
    sites.as_ref().is_none_or(|sites| {
        sites
            .iter()
            .any(|s| host == *s || host.ends_with(&format!(".{s}")))
    })
}

/// The spec's `Permits(InEffect(limits), act)`.
fn permits(intent: &Intent, now: SystemTime, act: Act) -> bool {
    let none = Allow::default();
    let e = if intent.in_force(now) {
        &intent.allow
    } else {
        &none
    };
    let app_ok = |n: &str| !is_password_manager(n) && e.apps.as_ref().is_none_or(|a| a.contains(n));
    match act {
        Act::Look => true,
        Act::Input(None) => !e.read_only && e.apps.is_none(),
        Act::Input(Some(i)) => !e.read_only && app_ok(APPS[i]),
        Act::Launch(i) => !e.read_only && !is_browser(APPS[i]) && app_ok(APPS[i]),
        Act::Open(i) => !e.read_only && app_ok(BROWSER) && site_ok(&e.sites, HOSTS[i]),
    }
}

fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn limits_keep_to_the_spec(steps in prop::collection::vec(step(), 1..40)) {
        let mut now = t0();
        let mut limits = Limits::none();
        let mut intent = Intent::default();
        for step in steps {
            match step {
                Step::Set(allow) => {
                    limits = Limits::set(&allow, now);
                    intent = Intent { allow: allow.clone(), at: Some(now) };
                    let limits_something = allow.apps.is_some() || allow.sites.is_some() || allow.read_only;
                    let until = if limits_something { allow.duration.map(|d| now + d) } else { None };
                    prop_assert_eq!(limits.until(), until);
                }
                Step::Off => {
                    limits = Limits::none();
                    intent = Intent::default();
                }
                Step::Tick(secs) => now += Duration::from_secs(secs),
                Step::Call(act) => {
                    let got = limits.check(act.action(), now);
                    let allowed = permits(&intent, now, act);
                    // CallsKeepToLimits and RefusedOnlyIfForbidden: admitted exactly when the limits in force
                    // at this call allow it.
                    prop_assert_eq!(got.is_ok(), allowed, "{:?} at +{:?} under {:?}: {:?}",
                        act, now.duration_since(t0()), intent, got);
                    if let Err(why) = &got {
                        let text = why.to_string().to_lowercase();
                        match why {
                            Refusal::Browser { .. } => prop_assert!(text.contains("open_url"), "{}", text),
                            _ => prop_assert!(text.contains("ask the user"), "{:?}: {}", why, text),
                        }
                    }
                    // PasswordManagerNeverUsed.
                    if let Act::Input(Some(i)) | Act::Launch(i) = act
                        && is_password_manager(APPS[i])
                    {
                        prop_assert!(matches!(got, Err(Refusal::PasswordManager { .. })), "{:?}: {:?}", act, got);
                    }
                    // BrowserOnlyThroughOpenUrl.
                    if let Act::Launch(i) = act
                        && is_browser(APPS[i])
                        && !is_password_manager(APPS[i])
                    {
                        prop_assert!(matches!(got, Err(Refusal::Browser { .. })), "{:?}: {:?}", act, got);
                    }
                    // ReadOnlyOnlyLooks.
                    if intent.in_force(now) && intent.allow.read_only && !matches!(act, Act::Look) {
                        prop_assert!(got.is_err(), "read-only let {:?} through", act);
                    }
                    // NoLimitAllowsAll.
                    if !intent.in_force(now) {
                        prop_assert!(!limits.in_force(now));
                        let excused = match act {
                            Act::Input(Some(i)) => is_password_manager(APPS[i]),
                            Act::Launch(i) => is_password_manager(APPS[i]) || is_browser(APPS[i]),
                            _ => false,
                        };
                        prop_assert!(excused || got.is_ok(), "no limit is in force, yet {:?} was refused: {:?}", act, got);
                    } else {
                        prop_assert!(limits.in_force(now));
                    }
                }
                Step::Fetch(i) => {
                    // OnlyListedSitesReached, and no more: the proxy lets a request through exactly when the
                    // sites in force list its host.
                    let none = Allow::default();
                    let e = if intent.in_force(now) { &intent.allow } else { &none };
                    prop_assert_eq!(limits.allows_site(HOSTS[i], now), site_ok(&e.sites, HOSTS[i]),
                        "{} at +{:?} under {:?}", HOSTS[i], now.duration_since(t0()), intent);
                }
            }
        }
    }
}
