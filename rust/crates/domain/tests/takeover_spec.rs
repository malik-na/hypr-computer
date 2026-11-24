//! Taking over agents' screens against the properties of specs/TakeOver.tla, over random sequences of events
//! for several sessions, arriving in any order, including ones the spec doesn't allow in a state. The API is
//! described at the top of tests/takeover.rs.
//!
//! The test plays the world the way a correct caller does: a session's screen is up from its `Connect` to its
//! `Disconnect`, a call the agent made is held from its `Wait` to its `Resume`, and the clipboards are joined
//! from a `TakenOver` to the next `Given`.
#![forbid(unsafe_code)]

use domain::takeover::{Call, Control, Effect, Event, Key, TakeOvers};
use proptest::prelude::*;

type Id = u32;

/// Session slots, as the spec's `Sessions`.
const SLOTS: usize = 3;

#[derive(Clone, Copy, Debug)]
enum Kind {
    Connect,
    Disconnect,
    ToolCall,
    Finish,
    Click,
    Pointer,
    Escape,
    OtherKey,
    Take,
    Give,
}

const KINDS: [Kind; 10] = [
    Kind::Connect,
    Kind::Disconnect,
    Kind::ToolCall,
    Kind::Finish,
    Kind::Click,
    Kind::Pointer,
    Kind::Escape,
    Kind::OtherKey,
    Kind::Take,
    Kind::Give,
];

/// One slot of the world.
#[derive(Clone, Copy, Default)]
struct Slot {
    /// The open session's id: its screen is up.
    open: Option<Id>,
    /// The last id this slot had, for late events from a session that is gone.
    last: Id,
    /// The agent has a call that was told to wait and hasn't been resumed.
    waiting: bool,
    /// The clipboards are joined (the spec's `clip`): from `TakenOver` to `Given`.
    joined: bool,
}

/// The session an event is about, if any.
fn about(event: Event<Id>) -> Option<Id> {
    match event {
        Event::Connect(k)
        | Event::Disconnect(k)
        | Event::ToolCall(k)
        | Event::Finish(k)
        | Event::Click(k)
        | Event::Pointer(k)
        | Event::Key(k, _)
        | Event::Take(k) => Some(k),
        Event::Give => None,
    }
}

fn names(effect: Effect<Id>) -> Id {
    match effect {
        Effect::Admit(k)
        | Effect::Wait(k)
        | Effect::Resume(k)
        | Effect::Forward(k)
        | Effect::TakenOver(k)
        | Effect::Given(k) => k,
    }
}

/// Whether `event` is one that may hand a screen back.
fn hands_back(event: Event<Id>) -> bool {
    matches!(
        event,
        Event::Give
            | Event::Key(_, Key::Escape)
            | Event::Click(_)
            | Event::Take(_)
            | Event::Disconnect(_)
    )
}

/// TakeOverTakesEffect, bounded: a session the user clicked into is theirs once its running call finishes.
fn take_over_takes_effect(t: &TakeOvers<Id>, id: Id) -> Result<(), TestCaseError> {
    let Some(before) = t.get(id) else {
        return Ok(());
    };
    if before.control != Control::Taking {
        return Ok(());
    }
    prop_assert_eq!(
        before.call,
        Call::Running,
        "TakeOverTakesEffect: session {} is taking with no call to wait for: {:?}",
        id,
        before
    );
    let mut t = t.clone();
    let effects = t.step(Event::Finish(id));
    prop_assert_eq!(
        effects,
        vec![Effect::TakenOver(id)],
        "TakeOverTakesEffect: the call ended but the user didn't get the screen, from {:?}",
        before
    );
    prop_assert_eq!(
        t.get(id).map(|s| (s.control, s.call)),
        Some((Control::User, Call::Idle)),
        "TakeOverTakesEffect, from {:?}",
        before
    );
    Ok(())
}

/// Escape twice, probed: a user who has the screen hands it back with two Escapes, the second going to no one,
/// and a waiting call is resumed by it.
fn escape_twice_hands_back(t: &TakeOvers<Id>, id: Id) -> Result<(), TestCaseError> {
    let Some(before) = t.get(id) else {
        return Ok(());
    };
    if before.control != Control::User {
        return Ok(());
    }
    let mut t = t.clone();
    let first = t.step(Event::Key(id, Key::Escape));
    let effects = if first.contains(&Effect::Given(id)) {
        first
    } else {
        prop_assert_eq!(
            first,
            vec![Effect::Forward(id)],
            "the first Escape, in {:?}",
            before
        );
        t.step(Event::Key(id, Key::Escape))
    };
    let mut want = vec![Effect::Given(id)];
    if before.call == Call::Waiting {
        want.push(Effect::Resume(id));
    }
    let mut got = effects.clone();
    got.sort_by_key(|e| format!("{e:?}"));
    want.sort_by_key(|e| format!("{e:?}"));
    prop_assert_eq!(
        got,
        want,
        "WaitingOnlyWhileTakenOver: Escape twice didn't hand back cleanly, from {:?}",
        before
    );
    prop_assert_eq!(
        t.taken(),
        None,
        "Escape twice left a take-over, from {:?}",
        before
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    #[test]
    fn every_order_of_events_keeps_the_specs_properties(
        picks in proptest::collection::vec((0..KINDS.len(), 0..SLOTS), 0..96)
    ) {
        let mut t: TakeOvers<Id> = TakeOvers::new();
        let mut world = [Slot::default(); SLOTS];
        let mut next_id: Id = 1;

        for (kind, slot) in picks {
            let target = world[slot].open.unwrap_or(world[slot].last);
            let event = match KINDS[kind] {
                Kind::Connect => {
                    let id = world[slot].open.unwrap_or(next_id);
                    if world[slot].open.is_none() {
                        next_id += 1;
                        world[slot] = Slot { open: Some(id), last: id, ..Slot::default() };
                    }
                    Event::Connect(id)
                }
                Kind::Disconnect => Event::Disconnect(target),
                Kind::ToolCall => Event::ToolCall(target),
                Kind::Finish => Event::Finish(target),
                Kind::Click => Event::Click(target),
                Kind::Pointer => Event::Pointer(target),
                Kind::Escape => Event::Key(target, Key::Escape),
                Kind::OtherKey => Event::Key(target, Key::Other),
                Kind::Take => Event::Take(target),
                Kind::Give => Event::Give,
            };

            let before = t.clone();
            let taken_before = before.taken();
            let effects = t.step(event);

            // The world, as a correct caller makes it.
            for &effect in &effects {
                let k = names(effect);
                let Some(s) = world.iter_mut().find(|s| s.open == Some(k)) else {
                    return Err(TestCaseError::fail(format!("{effect:?} for a session that isn't open, on {event:?}")));
                };
                match effect {
                    Effect::Wait(_) => {
                        prop_assert!(!s.waiting, "a second call held for session {}", k);
                        s.waiting = true;
                    }
                    Effect::Resume(_) => {
                        // ResumedAtMostOnce: only a held call is resumed, and only once.
                        prop_assert!(s.waiting, "ResumedAtMostOnce: {:?} with no call held, on {:?}", effect, event);
                        s.waiting = false;
                    }
                    Effect::TakenOver(_) => {
                        prop_assert!(!s.joined, "TakenOver twice for session {} without a Given", k);
                        s.joined = true;
                    }
                    Effect::Given(_) => s.joined = false,
                    Effect::Admit(_) | Effect::Forward(_) => {}
                }
            }
            if let Event::Disconnect(id) = event
                && world[slot].open == Some(id)
            {
                world[slot] = Slot { open: None, last: id, ..Slot::default() };
            }

            // Every effect is about the event's own session, or the screen taken over before it, which a click,
            // a take or a give hands back.
            for &effect in &effects {
                let k = names(effect);
                let own = about(event) == Some(k);
                let handed_back = taken_before == Some(k)
                    && matches!(event, Event::Click(_) | Event::Take(_) | Event::Give)
                    && matches!(effect, Effect::Given(_) | Effect::Resume(_));
                prop_assert!(own || handed_back, "an effect for another session: {:?} gave {:?}", event, effects);
            }
            // Each effect answers an event it may answer.
            for &effect in &effects {
                let fits = match effect {
                    Effect::Admit(_) | Effect::Wait(_) => matches!(event, Event::ToolCall(_)),
                    Effect::Resume(_) | Effect::Given(_) => hands_back(event),
                    Effect::Forward(_) => matches!(event, Event::Click(_) | Event::Pointer(_) | Event::Key(..)),
                    Effect::TakenOver(_) => matches!(event, Event::Click(_) | Event::Take(_) | Event::Finish(_)),
                };
                prop_assert!(fits, "an effect that doesn't answer its event: {:?} gave {:?}", event, effects);
            }

            // The agent's call: run when the agent drives the screen, held otherwise; one at a time.
            if let Event::ToolCall(id) = event
                && let Some(was) = before.get(id)
            {
                let want = match (was.call, was.control) {
                    (Call::Idle, Control::Agent) => vec![Effect::Admit(id)],
                    (Call::Idle, _) => vec![Effect::Wait(id)],
                    _ => vec![],
                };
                prop_assert_eq!(&effects, &want, "AgentAndUserNeverInterleave: a call in {:?}", was);
            }
            for &effect in &effects {
                match effect {
                    // AgentAndUserNeverInterleave: the agent's calls run only while it drives its screen ...
                    Effect::Admit(k) | Effect::Resume(k) => {
                        let now = t.get(k);
                        prop_assert_eq!(now.map(|n| (n.control, n.call)), Some((Control::Agent, Call::Running)),
                            "AgentAndUserNeverInterleave: {:?} on {:?}", effect, event);
                    }
                    // ... and the user's input reaches a screen only while the user has it, with no call running.
                    Effect::Forward(k) => {
                        let was = before.get(k);
                        let now = t.get(k);
                        prop_assert_eq!(was.map(|w| w.control), Some(Control::User),
                            "the user's input reached a screen they hadn't taken over, on {:?}", event);
                        prop_assert!(now.is_some_and(|n| n.control == Control::User && n.call != Call::Running),
                            "AgentAndUserNeverInterleave: {:?} forwarded with {:?}", event, now);
                        prop_assert_eq!(&effects, &vec![Effect::Forward(k)], "forwarded and more, on {:?}", event);
                    }
                    _ => {}
                }
            }
            // Watch-only: in a view the user hasn't taken over, only a click does anything.
            if let Event::Pointer(id) | Event::Key(id, _) = event
                && before.get(id).is_some_and(|w| w.control != Control::User)
            {
                prop_assert_eq!(&effects, &vec![], "input in a watch-only view did something: {:?}", event);
            }

            let mut taken = Vec::new();
            for s in &world {
                let Some(id) = s.open else { continue };
                let Some(now) = t.get(id) else {
                    return Err(TestCaseError::fail(format!("session {id} forgotten on {event:?} while its screen is up")));
                };
                if now.control != Control::Agent {
                    taken.push(id);
                }
                prop_assert!(!(now.control == Control::User && now.call == Call::Running),
                    "AgentAndUserNeverInterleave: {:?} after {:?}", now, event);
                prop_assert!(now.call != Call::Waiting || now.control != Control::Agent,
                    "WaitingOnlyWhileTakenOver: {:?} after {:?}", now, event);
                prop_assert_eq!(s.waiting, now.call == Call::Waiting,
                    "WaitingOnlyWhileTakenOver: held {} but {:?}, after {:?}", s.waiting, now, event);
                prop_assert_eq!(s.joined, now.control == Control::User,
                    "ClipboardOnlyDuringTakeOver: clipboards joined {} with {:?}, after {:?}", s.joined, now, event);
                take_over_takes_effect(&t, id)?;
                escape_twice_hands_back(&t, id)?;
            }
            // OneTakeOverAtATime.
            prop_assert!(taken.len() <= 1, "OneTakeOverAtATime: {:?} taken over after {:?}", taken, event);
            prop_assert_eq!(t.taken(), taken.first().copied(), "taken() is wrong after {:?}", event);

            // EndedSessionLeavesNothing: a gone session is forgotten, and handed back if it was taken over.
            for s in &world {
                if s.open.is_none() {
                    prop_assert_eq!(t.get(s.last), None, "a gone session is still known after {:?}", event);
                }
            }
            if let Event::Disconnect(id) = event
                && let Some(was) = before.get(id)
            {
                let want = if was.control == Control::Agent { vec![] } else { vec![Effect::Given(id)] };
                prop_assert_eq!(&effects, &want, "EndedSessionLeavesNothing: disconnect in {:?}", was);
                prop_assert_ne!(t.taken(), Some(id), "EndedSessionLeavesNothing: a gone session holds the take-over");
            }

            for s in &world {
                let id = s.open.unwrap_or(s.last);
                let was = before.get(id).map(|w| w.control);
                let now = t.get(id).map(|n| n.control);
                // OnlyTheUserHandsBack.
                if was.is_some_and(|w| w != Control::Agent) && now.is_some_and(|n| n == Control::Agent) {
                    prop_assert!(hands_back(event) && !matches!(event, Event::Disconnect(_)),
                        "OnlyTheUserHandsBack: {:?} ended a take-over", event);
                    prop_assert!(effects.contains(&Effect::Given(id)), "handed back without Given, on {:?}", event);
                }
                // OnlyTheUserTakesOver.
                if was == Some(Control::Agent) && now.is_some_and(|n| n != Control::Agent) {
                    prop_assert!(matches!(event, Event::Click(k) | Event::Take(k) if k == id),
                        "OnlyTheUserTakesOver: {:?} took session {} over", event, id);
                }
            }

            // SessionsAreIndependent: a step of one session changes only it, and the screen taken over that a
            // click, a take or a give hands back.
            for s in &world {
                let other = s.open.unwrap_or(s.last);
                let mine = about(event) == Some(other);
                let handed_back = taken_before == Some(other)
                    && matches!(event, Event::Click(_) | Event::Take(_) | Event::Give);
                if !mine && !handed_back {
                    prop_assert_eq!(t.get(other), before.get(other), "SessionsAreIndependent: {:?} changed session {}", event, other);
                }
            }
        }
    }
}
