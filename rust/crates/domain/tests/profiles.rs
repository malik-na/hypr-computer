//! Saved, named browser profiles, tested at their seam: `domain::profiles`, against specs/Profiles.tla.
//!
//! # The API these tests fix
//!
//! `rust/crates/domain/src/profiles.rs`, declared `pub mod profiles;` in `lib.rs`. It holds what the daemon
//! believes about profiles: the spec's `uses`, `acquiring` and `holder`. The spec's `browser`, `orphan` and `exists`
//! are the world: whether a profile's folder is on disk is the caller's to look up, and it passes the answer in.
//!
//! ```ignore
//! /// A profile's name: 1 to 64 ASCII letters, digits, '.', '_' and '-', the first a letter or a digit. So it is
//! /// always one plain folder name: never "", "." or "..", never hidden, never holding a '/'.
//! #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
//! pub struct ProfileName(/* private */);
//!
//! impl ProfileName {
//!     pub fn parse(name: &str) -> Result<ProfileName, BadName>;
//!     pub fn as_str(&self) -> &str;
//! }
//! impl std::fmt::Display for ProfileName { /* the name itself */ }
//!
//! /// Why a string isn't a profile's name, in words for the user or the agent: it says what a name may hold.
//! #[derive(Clone, Debug, PartialEq, Eq)]
//! pub struct BadName(/* private */);
//! impl std::fmt::Display for BadName { /* ... */ }
//! impl std::error::Error for BadName {}
//!
//! /// The profile a session's browser runs on.
//! #[derive(Clone, Debug, PartialEq, Eq)]
//! pub enum Choice { Fresh, Named(ProfileName) }
//!
//! /// A session's profile: the spec's `uses`, with `acquiring` as `Acquiring`.
//! /// `Unset`: no `open_url` has fixed it yet. `Acquiring(p)`: the table gives `p` to the session, and the
//! /// caller is taking its OS lock. `Named(p)`: the session has `p`, OS lock and all.
//! #[derive(Clone, Debug, PartialEq, Eq)]
//! pub enum Uses { Unset, Fresh, Acquiring(ProfileName), Named(ProfileName) }
//!
//! /// Something that happened, or is asked for, in session `K` (the daemon uses `SessionId`).
//! #[derive(Clone, Debug, PartialEq, Eq)]
//! pub enum Event<K> {
//!     Connect(K),
//!     /// `open_url`. `profile`: the profile it names, if any. `on_disk`: whether that profile's folder exists
//!     /// now (ignored without a profile).
//!     Open { id: K, profile: Option<ProfileName>, on_disk: bool },
//!     /// The OS lock asked for by `Effect::Acquire` is taken.
//!     Acquired(K),
//!     /// The OS lock asked for by `Effect::Acquire` is held by someone else: a browser of a daemon that died.
//!     Busy(K),
//!     Disconnect(K),
//!     /// The session's teardown is done: its browser is seen to be gone.
//!     TeardownDone(K),
//!     /// `hypr-computer profile delete <name>`, while the daemon runs.
//!     Delete(ProfileName),
//! }
//!
//! /// Why an `open_url` naming a profile was refused.
//! #[derive(Clone, Debug, PartialEq, Eq)]
//! pub enum Refusal {
//!     /// No profile has that name: the user makes one with `hypr-computer profile new <name>`.
//!     Missing(ProfileName),
//!     /// Another session has it.
//!     InUse(ProfileName),
//!     /// The session's profile is already fixed, to another one or to a fresh one: one profile per session.
//!     Fixed { has: Choice, asked: ProfileName },
//! }
//!
//! /// What the caller must do because of an event.
//! #[derive(Clone, Debug, PartialEq, Eq)]
//! pub enum Effect<K> {
//!     /// Take the profile's OS lock, then step `Acquired(k)`, or `Busy(k)` if it is held.
//!     Acquire(K, ProfileName),
//!     /// Open the page in the session's browser, starting it on that profile if it isn't running.
//!     Browse(K, Choice),
//!     /// Refuse the `open_url`, and tell the agent why.
//!     Refuse(K, Refusal),
//!     /// Drop the session's OS lock on the profile (if the caller holds it).
//!     Release(K, ProfileName),
//!     /// No session has the profile: delete its folder, if its OS lock can be taken (else refuse, as in use).
//!     Delete(ProfileName),
//!     /// A session has the profile: refuse the delete, and delete nothing.
//!     RefuseDelete(ProfileName),
//! }
//!
//! /// Every session's profile, and which session each profile is given to (the spec's `holder`).
//! #[derive(Clone, Debug)]          // PartialEq, Eq and Default are welcome, not required
//! pub struct Profiles<K> { /* private */ }
//!
//! impl<K: Copy + Ord> Profiles<K> {
//!     /// No session, no profile in use.
//!     pub fn new() -> Self;
//!     /// Applies one event, and says what to do about it. Pure: no I/O, no clock.
//!     pub fn step(&mut self, event: Event<K>) -> Vec<Effect<K>>;
//!     /// The session's profile; `None` before its `Connect` and after its `TeardownDone`.
//!     pub fn uses(&self, id: K) -> Option<Uses>;
//!     /// The session the table gives the profile to, if any.
//!     pub fn holder(&self, name: &ProfileName) -> Option<K>;
//! }
//! ```
//!
//! `step`, for a session `k` whose profile is `u` before the event. "Ending": `k` disconnected, its teardown not
//! done. Every case not listed changes nothing and returns no effect: an event for an unknown `k`, a `Connect` of
//! a known one, `Open` while `Acquiring` or ending, `Acquired`/`Busy` unless `Acquiring`, `Disconnect` while
//! ending, `TeardownDone` unless ending.
//!
//! | Event | When | Then | Effects |
//! | --- | --- | --- | --- |
//! | `Connect(k)` | `k` unknown | `Unset` | none |
//! | `Open` without a profile | `Unset` | `Fresh` | `Browse(k, Fresh)` |
//! | `Open` without a profile | `Fresh` or `Named(p)` | unchanged | `Browse(k, Fresh)` or `Browse(k, Named(p))` |
//! | `Open` naming `p` | `Named(p)` | unchanged | `Browse(k, Named(p))`, whatever `on_disk` |
//! | `Open` naming `q` | `Fresh` or `Named(p)`, `p` ≠ `q` | unchanged | `Refuse(k, Fixed { has, asked: q })` |
//! | `Open` naming `p` | `Unset`, not `on_disk` | unchanged | `Refuse(k, Missing(p))` |
//! | `Open` naming `p` | `Unset`, `on_disk`, another session holds `p` (ending ones too) | unchanged | `Refuse(k, InUse(p))` |
//! | `Open` naming `p` | `Unset`, `on_disk`, nobody holds `p` | `Acquiring(p)`; `k` holds `p` | `Acquire(k, p)` |
//! | `Acquired(k)` | `Acquiring(p)` | `Named(p)` | `Browse(k, Named(p))`; none if ending |
//! | `Busy(k)` | `Acquiring(p)` | `Unset`; nobody holds `p` | `Refuse(k, InUse(p))` |
//! | `Disconnect(k)` | not ending | ending; `u` and what `k` holds unchanged | none |
//! | `TeardownDone(k)` | ending | `k` forgotten; nobody holds what it held | `Release(k, p)` if `u` was `Acquiring(p)` or `Named(p)` |
//! | `Delete(p)` | a session holds `p` (ending ones too) | unchanged | `RefuseDelete(p)` |
//! | `Delete(p)` | nobody holds `p` | unchanged | `Delete(p)` |
//!
//! So a session holds `p` exactly while its profile is `Acquiring(p)` or `Named(p)`, from the `Open` that asks
//! for it until its `TeardownDone`: never released at `Disconnect`, while the browser may still be writing.
//!
//! What the caller must do, because `step` can't check it:
//! - **One `Profiles` per daemon, owned by one task, stepped one event at a time.** `Connect` a fresh id when a
//!   session is created and step `TeardownDone` once its teardown is done. Ids aren't reused.
//! - **Every `open_url` steps `Open`**, after looking whether the named profile's folder exists. On `Browse` it
//!   opens the page; on `Refuse` the agent gets a clear error. On `Acquire` it takes the profile's OS lock, a
//!   lock the session holds and the browser it starts inherits, so that the kernel frees it only when both are
//!   gone (a browser left running by a daemon that died still holds it). Then it steps `Acquired` or `Busy`.
//! - **`TeardownDone` is stepped after the session's browser is seen gone**, and after the answer to any
//!   `Acquire` in progress. On `Release` the caller drops the session's OS lock.
//! - **`hypr-computer profile delete`** steps `Delete` while the daemon runs. On `Delete(p)` the folder is
//!   deleted only if the OS lock can be taken; without a daemon the command checks the OS lock alone.
#![forbid(unsafe_code)]

use domain::profiles::{BadName, Choice, Effect, Event, ProfileName, Profiles, Refusal, Uses};

type Id = u32;

fn name(s: &str) -> ProfileName {
    let parsed = ProfileName::parse(s);
    assert!(
        parsed.is_ok(),
        "{s:?} should be a profile's name: {parsed:?}"
    );
    parsed.unwrap_or_else(|_| unreachable!())
}

fn connected(ids: &[Id]) -> Profiles<Id> {
    let mut profiles = Profiles::new();
    for &id in ids {
        assert_eq!(profiles.step(Event::Connect(id)), vec![]);
        assert_eq!(profiles.uses(id), Some(Uses::Unset));
    }
    profiles
}

fn open(profiles: &mut Profiles<Id>, id: Id, profile: Option<&str>) -> Vec<Effect<Id>> {
    profiles.step(Event::Open {
        id,
        profile: profile.map(name),
        on_disk: true,
    })
}

/// Session `id` opens a page in profile `p`, and gets it.
fn take(profiles: &mut Profiles<Id>, id: Id, p: &str) {
    assert_eq!(
        open(profiles, id, Some(p)),
        vec![Effect::Acquire(id, name(p))]
    );
    assert_eq!(profiles.uses(id), Some(Uses::Acquiring(name(p))));
    assert_eq!(profiles.holder(&name(p)), Some(id));
    assert_eq!(
        profiles.step(Event::Acquired(id)),
        vec![Effect::Browse(id, Choice::Named(name(p)))]
    );
    assert_eq!(profiles.uses(id), Some(Uses::Named(name(p))));
    assert_eq!(profiles.holder(&name(p)), Some(id));
}

#[test]
fn a_session_starts_fresh_unless_it_names_a_profile() {
    let mut profiles = connected(&[1]);
    assert_eq!(
        open(&mut profiles, 1, None),
        vec![Effect::Browse(1, Choice::Fresh)]
    );
    assert_eq!(profiles.uses(1), Some(Uses::Fresh));
    assert_eq!(
        open(&mut profiles, 1, None),
        vec![Effect::Browse(1, Choice::Fresh)]
    );
    assert_eq!(profiles.uses(1), Some(Uses::Fresh));
}

#[test]
fn a_session_that_names_a_profile_takes_it_and_keeps_it() {
    let mut profiles = connected(&[1]);
    take(&mut profiles, 1, "work");
    // Later pages open in it, named or not.
    assert_eq!(
        open(&mut profiles, 1, None),
        vec![Effect::Browse(1, Choice::Named(name("work")))]
    );
    assert_eq!(
        open(&mut profiles, 1, Some("work")),
        vec![Effect::Browse(1, Choice::Named(name("work")))]
    );
    assert_eq!(profiles.uses(1), Some(Uses::Named(name("work"))));
}

#[test]
fn a_sessions_profile_is_fixed_by_its_first_page() {
    let mut profiles = connected(&[1, 2]);
    take(&mut profiles, 1, "work");
    assert_eq!(
        open(&mut profiles, 1, Some("github")),
        vec![Effect::Refuse(
            1,
            Refusal::Fixed {
                has: Choice::Named(name("work")),
                asked: name("github"),
            }
        )]
    );
    assert_eq!(profiles.uses(1), Some(Uses::Named(name("work"))));
    assert_eq!(profiles.holder(&name("github")), None);

    // A fresh session can't switch to a named profile either.
    open(&mut profiles, 2, None);
    assert_eq!(
        open(&mut profiles, 2, Some("github")),
        vec![Effect::Refuse(
            2,
            Refusal::Fixed {
                has: Choice::Fresh,
                asked: name("github"),
            }
        )]
    );
    assert_eq!(profiles.uses(2), Some(Uses::Fresh));
    assert_eq!(profiles.holder(&name("github")), None);
}

#[test]
fn a_profile_that_isnt_on_disk_is_refused_and_fixes_nothing() {
    let mut profiles = connected(&[1]);
    let missing = profiles.step(Event::Open {
        id: 1,
        profile: Some(name("nope")),
        on_disk: false,
    });
    assert_eq!(
        missing,
        vec![Effect::Refuse(1, Refusal::Missing(name("nope")))]
    );
    assert_eq!(profiles.uses(1), Some(Uses::Unset));
    assert_eq!(profiles.holder(&name("nope")), None);
    // The session goes on, and may still pick.
    take(&mut profiles, 1, "work");
}

#[test]
fn a_second_session_cant_have_a_profile_in_use_and_the_first_keeps_it() {
    let mut profiles = connected(&[1, 2]);
    take(&mut profiles, 1, "work");
    assert_eq!(
        open(&mut profiles, 2, Some("work")),
        vec![Effect::Refuse(2, Refusal::InUse(name("work")))]
    );
    assert_eq!(profiles.uses(2), Some(Uses::Unset));
    assert_eq!(profiles.uses(1), Some(Uses::Named(name("work"))));
    assert_eq!(profiles.holder(&name("work")), Some(1));
    // The refused session may go fresh, or take another profile.
    take(&mut profiles, 2, "github");
}

/// TLC's counterexample for TwoSessionsBothAcquire: two sessions ask for a profile at once. The first holds it
/// from its `Open`, before its lock is taken, so the second is refused.
#[test]
fn two_sessions_that_ask_at_once_dont_both_get_it() {
    let mut profiles = connected(&[1, 2]);
    assert_eq!(
        open(&mut profiles, 1, Some("work")),
        vec![Effect::Acquire(1, name("work"))]
    );
    assert_eq!(
        open(&mut profiles, 2, Some("work")),
        vec![Effect::Refuse(2, Refusal::InUse(name("work")))]
    );
    assert_eq!(
        profiles.step(Event::Acquired(1)),
        vec![Effect::Browse(1, Choice::Named(name("work")))]
    );
    assert_eq!(profiles.step(Event::Acquired(2)), vec![]);
    assert_eq!(profiles.uses(2), Some(Uses::Unset));
    assert_eq!(profiles.holder(&name("work")), Some(1));
}

/// TLC's counterexample for ReleaseAtDisconnect: the first agent disconnects while its browser is still shutting
/// down. The profile stays its own until the teardown is done.
#[test]
fn a_profile_is_released_when_the_teardown_is_done_not_at_disconnect() {
    let mut profiles = connected(&[1, 2]);
    take(&mut profiles, 1, "work");
    assert_eq!(profiles.step(Event::Disconnect(1)), vec![]);
    assert_eq!(profiles.holder(&name("work")), Some(1));
    assert_eq!(profiles.uses(1), Some(Uses::Named(name("work"))));
    assert_eq!(
        open(&mut profiles, 2, Some("work")),
        vec![Effect::Refuse(2, Refusal::InUse(name("work")))]
    );

    assert_eq!(
        profiles.step(Event::TeardownDone(1)),
        vec![Effect::Release(1, name("work"))]
    );
    assert_eq!(profiles.uses(1), None);
    assert_eq!(profiles.holder(&name("work")), None);
    take(&mut profiles, 2, "work");
}

#[test]
fn a_lock_held_outside_the_table_refuses_the_page_and_lets_the_session_ask_again() {
    let mut profiles = connected(&[1]);
    assert_eq!(
        open(&mut profiles, 1, Some("work")),
        vec![Effect::Acquire(1, name("work"))]
    );
    assert_eq!(
        profiles.step(Event::Busy(1)),
        vec![Effect::Refuse(1, Refusal::InUse(name("work")))]
    );
    assert_eq!(profiles.uses(1), Some(Uses::Unset));
    assert_eq!(profiles.holder(&name("work")), None);
    // The old browser has exited since: this time it works.
    take(&mut profiles, 1, "work");
}

#[test]
fn deleting_a_profile_is_refused_while_a_session_holds_it() {
    let mut profiles = connected(&[1]);
    assert_eq!(
        profiles.step(Event::Delete(name("work"))),
        vec![Effect::Delete(name("work"))]
    );

    // While its lock is being taken, while the session has it, and while the session ends.
    open(&mut profiles, 1, Some("work"));
    assert_eq!(
        profiles.step(Event::Delete(name("work"))),
        vec![Effect::RefuseDelete(name("work"))]
    );
    profiles.step(Event::Acquired(1));
    assert_eq!(
        profiles.step(Event::Delete(name("work"))),
        vec![Effect::RefuseDelete(name("work"))]
    );
    profiles.step(Event::Disconnect(1));
    assert_eq!(
        profiles.step(Event::Delete(name("work"))),
        vec![Effect::RefuseDelete(name("work"))]
    );
    // Another profile may go meanwhile.
    assert_eq!(
        profiles.step(Event::Delete(name("github"))),
        vec![Effect::Delete(name("github"))]
    );
    // Refusing changed nothing.
    assert_eq!(profiles.holder(&name("work")), Some(1));
    assert_eq!(profiles.uses(1), Some(Uses::Named(name("work"))));

    profiles.step(Event::TeardownDone(1));
    assert_eq!(
        profiles.step(Event::Delete(name("work"))),
        vec![Effect::Delete(name("work"))]
    );
}

#[test]
fn a_session_that_disconnects_while_its_lock_is_taken_opens_nothing_and_releases_it() {
    let mut profiles = connected(&[1]);
    open(&mut profiles, 1, Some("work"));
    assert_eq!(profiles.step(Event::Disconnect(1)), vec![]);
    assert_eq!(profiles.step(Event::Acquired(1)), vec![]);
    assert_eq!(profiles.holder(&name("work")), Some(1));
    assert_eq!(
        profiles.step(Event::TeardownDone(1)),
        vec![Effect::Release(1, name("work"))]
    );
    assert_eq!(profiles.holder(&name("work")), None);
}

#[test]
fn a_fresh_or_unset_session_releases_nothing() {
    let mut profiles = connected(&[1, 2]);
    open(&mut profiles, 1, None);
    for id in [1, 2] {
        assert_eq!(profiles.step(Event::Disconnect(id)), vec![]);
        assert_eq!(profiles.step(Event::TeardownDone(id)), vec![]);
        assert_eq!(profiles.uses(id), None);
    }
}

#[test]
fn an_ending_session_opens_nothing() {
    let mut profiles = connected(&[1, 2]);
    take(&mut profiles, 1, "work");
    profiles.step(Event::Disconnect(1));
    assert_eq!(open(&mut profiles, 1, None), vec![]);
    assert_eq!(open(&mut profiles, 1, Some("work")), vec![]);
    profiles.step(Event::Disconnect(2));
    assert_eq!(open(&mut profiles, 2, Some("github")), vec![]);
    assert_eq!(profiles.uses(2), Some(Uses::Unset));
    assert_eq!(profiles.holder(&name("github")), None);
}

#[test]
fn events_out_of_place_change_nothing() {
    let mut profiles = connected(&[1]);
    take(&mut profiles, 1, "work");
    let before = (profiles.uses(1), profiles.holder(&name("work")));
    for event in [
        Event::Connect(1),
        Event::Acquired(1),
        Event::Busy(1),
        Event::TeardownDone(1),
        Event::Acquired(9),
        Event::Busy(9),
        Event::Disconnect(9),
        Event::TeardownDone(9),
        Event::Open {
            id: 9,
            profile: Some(name("work")),
            on_disk: true,
        },
    ] {
        assert_eq!(profiles.step(event.clone()), vec![], "{event:?}");
        assert_eq!(
            (profiles.uses(1), profiles.holder(&name("work"))),
            before,
            "{event:?}"
        );
        assert_eq!(profiles.uses(9), None, "{event:?}");
    }
    // A second Open while the first one's lock is being taken does nothing either.
    let mut profiles = connected(&[2]);
    open(&mut profiles, 2, Some("work"));
    assert_eq!(open(&mut profiles, 2, None), vec![]);
    assert_eq!(open(&mut profiles, 2, Some("work")), vec![]);
    assert_eq!(profiles.uses(2), Some(Uses::Acquiring(name("work"))));
}

#[test]
fn sessions_dont_change_each_other() {
    let mut profiles = connected(&[1, 2, 3]);
    take(&mut profiles, 1, "work");
    open(&mut profiles, 2, None);
    take(&mut profiles, 3, "github");
    profiles.step(Event::Disconnect(3));
    profiles.step(Event::TeardownDone(3));
    assert_eq!(profiles.uses(1), Some(Uses::Named(name("work"))));
    assert_eq!(profiles.uses(2), Some(Uses::Fresh));
    assert_eq!(profiles.holder(&name("work")), Some(1));
    assert_eq!(profiles.holder(&name("github")), None);
}

#[test]
fn profile_names_are_plain_folder_names() {
    let longest = "a".repeat(64);
    for ok in [
        "work",
        "GitHub-2",
        "a.b_c",
        "0",
        "x-",
        "x.",
        longest.as_str(),
    ] {
        let parsed = ProfileName::parse(ok);
        assert!(parsed.is_ok(), "{ok:?} was refused: {parsed:?}");
        let parsed = parsed.unwrap_or_else(|_| unreachable!());
        assert_eq!(parsed.as_str(), ok);
        assert_eq!(parsed.to_string(), ok);
    }
    let too_long = "a".repeat(65);
    for bad in [
        "",
        ".",
        "..",
        ".hidden",
        "-x",
        "_x",
        "../x",
        "a/b",
        "/abs",
        "a b",
        "a\nb",
        "naïve",
        "a\0b",
        "a\\b",
        "~",
        too_long.as_str(),
    ] {
        let parsed: Result<ProfileName, BadName> = ProfileName::parse(bad);
        match parsed {
            Ok(n) => panic!("{bad:?} was taken as the name {n:?}"),
            Err(why) => {
                let text = why.to_string();
                assert!(
                    text.contains("letters") && text.contains("digits"),
                    "the error for {bad:?} should say what a name may hold (letters, digits, ...): {text:?}"
                );
            }
        }
    }
}

#[test]
fn names_compare_by_their_text() {
    assert_eq!(name("work"), name("work"));
    assert_ne!(name("work"), name("Work"));
    let mut names = vec![name("b"), name("a"), name("c")];
    names.sort();
    assert_eq!(names, vec![name("a"), name("b"), name("c")]);
}
