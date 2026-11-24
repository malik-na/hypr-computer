//! Saved profiles against the properties of specs/Profiles.tla, over random sequences of events for several
//! sessions and profiles, daemon restarts included, arriving in any order, including ones the spec doesn't allow
//! in a state. The API is described at the top of tests/profiles.rs.
//!
//! The test plays the world the way a correct caller does: it looks on disk for a profile before `Open`, takes
//! a profile's OS lock on `Acquire` (an exclusive lock that a browser inherits, so a dead daemon's browser keeps
//! holding it until it exits), starts or reuses the session's browser on `Browse`, closes an ending session's
//! browser before stepping `TeardownDone`, and deletes a folder on `Delete` only if its OS lock is free.
#![forbid(unsafe_code)]

use domain::profiles::{Choice, Effect, Event, ProfileName, Profiles, Refusal, Uses};
use proptest::prelude::*;

type Id = u32;

/// Session slots, as the spec's `Sessions`.
const SLOTS: usize = 3;
/// Profile names, as the spec's `Profiles`.
const NAMES: [&str; 2] = ["work", "github"];

fn name(i: usize) -> ProfileName {
    match ProfileName::parse(NAMES[i]) {
        Ok(n) => n,
        Err(_) => unreachable!("{} is a profile's name", NAMES[i]),
    }
}

fn index(n: &ProfileName) -> usize {
    NAMES
        .iter()
        .position(|s| *s == n.as_str())
        .unwrap_or_else(|| unreachable!("an unknown profile {n:?}"))
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Connect,
    OpenFresh,
    Open,
    OpenMissing,
    Answer,
    BrowserCloses,
    Disconnect,
    BrowserStopped,
    TeardownDone,
    Stray,
    Create,
    Delete,
    DaemonRestarts,
    OrphanExits,
}

const KINDS: [Kind; 14] = [
    Kind::Connect,
    Kind::OpenFresh,
    Kind::Open,
    Kind::OpenMissing,
    Kind::Answer,
    Kind::BrowserCloses,
    Kind::Disconnect,
    Kind::BrowserStopped,
    Kind::TeardownDone,
    Kind::Stray,
    Kind::Create,
    Kind::Delete,
    Kind::DaemonRestarts,
    Kind::OrphanExits,
];

/// One slot of the world.
#[derive(Clone, Debug, Default)]
struct Slot {
    /// The session's id, from its `Connect` until its `TeardownDone`.
    open: Option<Id>,
    /// The last id this slot had, for late events from a session that is gone.
    last: Id,
    ending: bool,
    /// The session's running browser, and the profile it runs on.
    browser: Option<Choice>,
    /// The profile whose OS lock the session is taking (an `Acquire` not yet answered).
    pending: Option<usize>,
    /// The profile whose OS lock the session holds.
    lock: Option<usize>,
}

#[derive(Clone, Debug, Default)]
struct World {
    slots: [Slot; SLOTS],
    on_disk: [bool; 2],
    /// A browser of a dead daemon still runs on the profile, holding its OS lock.
    orphan: [bool; 2],
}

impl World {
    /// Whether the profile's OS lock can be taken.
    fn lock_free(&self, p: usize) -> bool {
        !self.orphan[p] && self.slots.iter().all(|s| s.lock != Some(p))
    }
}

fn uses_profile(u: &Option<Uses>) -> Option<ProfileName> {
    match u {
        Some(Uses::Acquiring(p) | Uses::Named(p)) => Some(p.clone()),
        _ => None,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    #[test]
    fn every_order_of_events_keeps_the_specs_properties(
        picks in proptest::collection::vec((0..KINDS.len(), 0..SLOTS, 0..NAMES.len()), 0..120)
    ) {
        let mut profiles: Profiles<Id> = Profiles::new();
        let mut world = World::default();
        let mut next_id: Id = 1;

        for (kind, slot, p) in picks {
            let target = world.slots[slot].open.unwrap_or(world.slots[slot].last);
            let event = match KINDS[kind] {
                Kind::Connect => {
                    let s = &world.slots[slot];
                    if s.open.is_some() {
                        Event::Connect(target)
                    } else if s.browser.is_some() {
                        continue;
                    } else {
                        let id = next_id;
                        next_id += 1;
                        world.slots[slot] = Slot { open: Some(id), last: id, ..Slot::default() };
                        Event::Connect(id)
                    }
                }
                Kind::OpenFresh => Event::Open { id: target, profile: None, on_disk: false },
                Kind::Open => Event::Open { id: target, profile: Some(name(p)), on_disk: world.on_disk[p] },
                // A caller that looked too early: the folder appeared since. Treated as missing.
                Kind::OpenMissing => Event::Open { id: target, profile: Some(name(p)), on_disk: false },
                Kind::Answer => {
                    let Some(q) = world.slots[slot].pending else { continue };
                    world.slots[slot].pending = None;
                    if world.lock_free(q) {
                        world.slots[slot].lock = Some(q);
                        Event::Acquired(target)
                    } else {
                        Event::Busy(target)
                    }
                }
                Kind::BrowserCloses => {
                    let s = &mut world.slots[slot];
                    if s.open.is_none() || s.ending || s.browser.is_none() {
                        continue;
                    }
                    s.browser = None;
                    continue;
                }
                Kind::Disconnect => {
                    if world.slots[slot].open.is_some() {
                        world.slots[slot].ending = true;
                    }
                    Event::Disconnect(target)
                }
                Kind::BrowserStopped => {
                    let s = &mut world.slots[slot];
                    if s.ending && s.browser.is_some() {
                        s.browser = None;
                    }
                    continue;
                }
                Kind::TeardownDone => {
                    let s = &world.slots[slot];
                    // The caller waits for its browser to be gone, and for its lock's answer.
                    if s.open.is_some() && (!s.ending || s.browser.is_some() || s.pending.is_some()) {
                        continue;
                    }
                    Event::TeardownDone(target)
                }
                // Events a correct caller doesn't send, which must change nothing.
                Kind::Stray => match p {
                    0 if world.slots[slot].pending.is_none() => Event::Acquired(target),
                    1 if world.slots[slot].pending.is_none() => Event::Busy(target),
                    _ => Event::TeardownDone(next_id + 1000),
                },
                Kind::Create => {
                    world.on_disk[p] = true;
                    continue;
                }
                Kind::Delete => Event::Delete(name(p)),
                Kind::DaemonRestarts => {
                    for s in &mut world.slots {
                        if let Some(Choice::Named(n)) = &s.browser {
                            world.orphan[index(n)] = true;
                        }
                        let last = s.open.unwrap_or(s.last);
                        *s = Slot { last, ..Slot::default() };
                    }
                    profiles = Profiles::new();
                    continue;
                }
                Kind::OrphanExits => {
                    world.orphan[p] = false;
                    continue;
                }
            };

            let before = profiles.clone();
            let effects = profiles.step(event.clone());

            // The event's own session: effects speak only for it.
            let about = match &event {
                Event::Connect(k) | Event::Acquired(k) | Event::Busy(k) | Event::Disconnect(k) | Event::TeardownDone(k) => Some(*k),
                Event::Open { id, .. } => Some(*id),
                Event::Delete(_) => None,
            };
            for e in &effects {
                let k = match e {
                    Effect::Acquire(k, _) | Effect::Browse(k, _) | Effect::Refuse(k, _) | Effect::Release(k, _) => Some(*k),
                    Effect::Delete(_) | Effect::RefuseDelete(_) => None,
                };
                prop_assert_eq!(k, about, "an effect for another session: {:?} gave {:?}", event, effects);
            }
            prop_assert!(effects.len() <= 1, "{:?} gave several effects: {:?}", event, effects);

            // Events a correct caller doesn't send change nothing.
            if matches!(KINDS[kind], Kind::Stray) || before.uses(target).is_none() && about == Some(target) && !matches!(event, Event::Connect(_)) {
                prop_assert_eq!(&effects, &vec![], "{:?} out of place gave {:?}", event, effects);
                for s in &world.slots {
                    let id = s.open.unwrap_or(s.last);
                    prop_assert_eq!(profiles.uses(id), before.uses(id), "{:?} out of place changed session {}", event, id);
                }
            }

            // Play the effects, as a correct caller does.
            for e in &effects {
                match e {
                    Effect::Acquire(k, n) => {
                        let Some(s) = world.slots.iter_mut().find(|s| s.open == Some(*k)) else {
                            return Err(TestCaseError::fail(format!("{e:?} for a session that is gone")));
                        };
                        prop_assert!(matches!(&event, Event::Open { profile: Some(asked), on_disk: true, .. } if asked == n), "{:?} on {:?}", e, event);
                        prop_assert!(s.pending.is_none() && s.lock.is_none(), "{:?} while it has {:?}", e, s);
                        s.pending = Some(index(n));
                    }
                    Effect::Browse(k, choice) => {
                        let Some(s) = world.slots.iter_mut().find(|s| s.open == Some(*k)) else {
                            return Err(TestCaseError::fail(format!("{e:?} for a session that is gone")));
                        };
                        prop_assert!(!s.ending, "{:?} for a session that disconnected", e);
                        if let Choice::Named(n) = choice {
                            prop_assert_eq!(s.lock, Some(index(n)), "{:?} without the profile's OS lock", e);
                        }
                        s.browser = Some(choice.clone());
                    }
                    Effect::Refuse(_, why) => {
                        match (why, &event) {
                            (Refusal::Missing(n), Event::Open { profile: Some(asked), on_disk: false, .. }) => {
                                prop_assert_eq!(n, asked);
                            }
                            (Refusal::Missing(_), _) => prop_assert!(false, "{:?} on {:?}", why, event),
                            (Refusal::InUse(n), Event::Busy(_)) => {
                                prop_assert_eq!(uses_profile(&before.uses(target)), Some(n.clone()));
                            }
                            (Refusal::InUse(n), Event::Open { profile: Some(asked), .. }) => {
                                prop_assert_eq!(n, asked);
                                let other = before.holder(n);
                                prop_assert!(other.is_some() && other != Some(target), "InUse, but {:?} holds {:?}", other, n);
                            }
                            (Refusal::Fixed { has, asked }, Event::Open { profile: Some(a), .. }) => {
                                prop_assert_eq!(asked, a);
                                let was = before.uses(target);
                                let fixed = match has {
                                    Choice::Fresh => was == Some(Uses::Fresh),
                                    Choice::Named(h) => h != asked && was == Some(Uses::Named(h.clone())),
                                };
                                prop_assert!(fixed, "{:?}, but the session had {:?}", why, was);
                            }
                            _ => prop_assert!(false, "{:?} on {:?}", why, event),
                        }
                        prop_assert_eq!(profiles.uses(target), if matches!(event, Event::Busy(_)) { Some(Uses::Unset) } else { before.uses(target) }, "a refusal changed the session");
                    }
                    Effect::Release(k, n) => {
                        prop_assert_eq!(&event, &Event::TeardownDone(*k), "{:?} before the teardown was done", e);
                        let Some(s) = world.slots.iter_mut().find(|s| s.open == Some(*k)) else {
                            return Err(TestCaseError::fail(format!("{e:?} for a session that is gone")));
                        };
                        prop_assert_eq!(s.lock, Some(index(n)), "{:?}, but the session holds {:?}", e, s.lock);
                        prop_assert!(s.browser.is_none(), "{:?} while its browser runs", e);
                        s.lock = None;
                    }
                    Effect::Delete(n) => {
                        prop_assert_eq!(&event, &Event::Delete(n.clone()));
                        let q = index(n);
                        // DeleteInUse: nothing of the daemon's has it.
                        for s in &world.slots {
                            prop_assert!(s.lock != Some(q) && s.pending != Some(q), "NoBrowserOnDeletedProfile: {:?} while {:?}", e, s);
                        }
                        if world.lock_free(q) {
                            world.on_disk[q] = false;
                        }
                    }
                    Effect::RefuseDelete(n) => {
                        prop_assert_eq!(&event, &Event::Delete(n.clone()));
                        prop_assert!(before.holder(n).is_some(), "{:?}, but nobody holds it", e);
                    }
                }
            }
            if let Event::Delete(n) = &event {
                prop_assert_eq!(effects.len(), 1, "a delete wasn't answered: {:?}", effects);
                prop_assert_eq!(effects[0].clone(), if before.holder(n).is_some() { Effect::RefuseDelete(n.clone()) } else { Effect::Delete(n.clone()) });
            }
            // The world's end of a teardown.
            if let Event::TeardownDone(k) = event
                && let Some(s) = world.slots.iter_mut().find(|s| s.open == Some(k))
            {
                prop_assert!(s.lock.is_none(), "TeardownDone kept the session's OS lock: {:?}", s);
                let last = k;
                *s = Slot { last, ..Slot::default() };
            }
            // A Busy session may ask again.
            if let Event::Busy(k) = event && matches!(before.uses(k), Some(Uses::Acquiring(_))) {
                prop_assert_eq!(profiles.uses(k), Some(Uses::Unset), "Busy didn't unset the session");
            }

            // OneBrowserPerProfile.
            for (q, qn) in NAMES.iter().enumerate() {
                let on = world.slots.iter().filter(|s| s.browser == Some(Choice::Named(name(q)))).count() + usize::from(world.orphan[q]);
                prop_assert!(on <= 1, "OneBrowserPerProfile: {} browsers on {}, after {:?}", on, qn, event);
            }
            // NoBrowserOnDeletedProfile.
            for s in &world.slots {
                if let Some(Choice::Named(n)) = &s.browser {
                    prop_assert!(world.on_disk[index(n)], "NoBrowserOnDeletedProfile: a browser on {:?}, after {:?}", n, event);
                }
            }
            for (q, qn) in NAMES.iter().enumerate() {
                prop_assert!(!world.orphan[q] || world.on_disk[q], "NoBrowserOnDeletedProfile: an orphan on {}", qn);
            }
            for s in &world.slots {
                let Some(id) = s.open else {
                    prop_assert_eq!(profiles.uses(s.last), None, "a gone session is still known after {:?}", event);
                    continue;
                };
                let Some(now) = profiles.uses(id) else {
                    return Err(TestCaseError::fail(format!("session {id} forgotten on {event:?} before its teardown")));
                };
                // BrowserOnItsOwnProfile.
                match (&s.browser, &now) {
                    (None, _) | (Some(Choice::Fresh), Uses::Fresh) => {}
                    (Some(Choice::Named(b)), Uses::Named(u)) if b == u => {}
                    (b, u) => prop_assert!(false, "BrowserOnItsOwnProfile: a browser on {:?} for a session using {:?}", b, u),
                }
                // HolderChoseIt, and its converse: a session holds a profile exactly while it is acquiring or
                // using it, until its teardown is done (ReleaseAtDisconnect, TwoSessionsBothAcquire).
                for (q, qn) in NAMES.iter().enumerate() {
                    let chose = uses_profile(&Some(now.clone())) == Some(name(q));
                    prop_assert_eq!(profiles.holder(&name(q)) == Some(id), chose, "session {} uses {:?} but the table gives {} to {:?}, after {:?}", id, now, qn, profiles.holder(&name(q)), event);
                }
                // OneProfilePerSession.
                let was = before.uses(id);
                if let Some(was) = was && was != now {
                    let fine = match (&was, &now) {
                        (Uses::Unset, Uses::Fresh | Uses::Acquiring(_)) => true,
                        (Uses::Acquiring(a), Uses::Named(b)) => a == b,
                        (Uses::Acquiring(_), Uses::Unset) => matches!(event, Event::Busy(_)),
                        _ => false,
                    };
                    prop_assert!(fine, "OneProfilePerSession: {:?} -> {:?} on {:?}", was, now, event);
                }
            }
            for (q, qn) in NAMES.iter().enumerate() {
                if let Some(h) = profiles.holder(&name(q)) {
                    prop_assert!(world.slots.iter().any(|s| s.open == Some(h)), "HolderChoseIt: {} held by gone session {}", qn, h);
                }
            }
            // SessionsAreIndependent.
            if let Some(k) = about {
                for s in &world.slots {
                    let other = s.open.unwrap_or(s.last);
                    if other != k {
                        prop_assert_eq!(profiles.uses(other), before.uses(other), "SessionsAreIndependent: {:?} changed session {}", event, other);
                    }
                }
            }
            // EndedSessionReleases and OpenAnswers, bounded: an ending session whose browser is gone and whose
            // lock is answered releases everything at TeardownDone.
            for s in &world.slots {
                let Some(id) = s.open else { continue };
                if s.ending && s.browser.is_none() && s.pending.is_none() {
                    let mut after = profiles.clone();
                    after.step(Event::TeardownDone(id));
                    for (q, qn) in NAMES.iter().enumerate() {
                        prop_assert_ne!(after.holder(&name(q)), Some(id), "EndedSessionReleases: {} kept after its teardown", qn);
                    }
                }
            }
        }
    }

    /// A profile's name is 1 to 64 ASCII letters, digits, '.', '_' and '-', the first a letter or a digit.
    #[test]
    fn a_name_is_accepted_exactly_when_it_follows_the_rule(
        s in "[a-zA-Z0-9._/ \\-éA~\\\\]{0,70}"
    ) {
        let rule = (1..=64).contains(&s.len())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
        match ProfileName::parse(&s) {
            Ok(n) => {
                prop_assert!(rule, "{:?} was accepted", s);
                prop_assert_eq!(n.as_str(), s.as_str());
                prop_assert!(!n.as_str().contains('/'));
            }
            Err(e) => {
                prop_assert!(!rule, "{:?} was refused: {}", s, e);
                prop_assert!(!e.to_string().is_empty());
            }
        }
    }
}
