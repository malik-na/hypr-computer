//! Stopping agents against the properties of specs/Stop.tla, over random sequences of events for several
//! sessions, arriving in any order, including ones the spec doesn't allow in a state. The API is described at the
//! top of tests/stop.rs.
//!
//! The test plays the world the way a correct caller does: a session's screen is up from its `Connect` to its
//! `Disconnect`, a button or key is held from a `Press` that was given until a `Release` or `Abort` is reported,
//! and the caller releases before it reports either.
#![forbid(unsafe_code)]

use domain::stop::{Call, Effect, Event, Run, Stops};
use proptest::prelude::*;

type Id = u32;

/// Session slots, as the spec's `Sessions`.
const SLOTS: usize = 3;

#[derive(Clone, Copy, Debug)]
enum Kind {
    Connect,
    Disconnect,
    ToolCall,
    Press,
    Move,
    Release,
    Finish,
    Abort,
    Stop,
    StopAll,
    Resume,
    ResumeAll,
}

const KINDS: [Kind; 12] = [
    Kind::Connect,
    Kind::Disconnect,
    Kind::ToolCall,
    Kind::Press,
    Kind::Move,
    Kind::Release,
    Kind::Finish,
    Kind::Abort,
    Kind::Stop,
    Kind::StopAll,
    Kind::Resume,
    Kind::ResumeAll,
];

/// One slot of the world: the spec's `session`/`screen`, `held` and `served`.
#[derive(Clone, Copy, Default)]
struct Slot {
    /// The open session's id: its screen is up.
    open: Option<Id>,
    /// The last id this slot had, for late events from a session that is gone.
    last: Id,
    held: bool,
    served: bool,
}

/// The session an event is about, if any.
fn about(event: Event<Id>) -> Option<Id> {
    match event {
        Event::Connect(k)
        | Event::Disconnect(k)
        | Event::ToolCall(k)
        | Event::Press(k)
        | Event::Move(k)
        | Event::Release(k)
        | Event::Finish(k)
        | Event::Abort(k)
        | Event::Stop(k)
        | Event::Resume(k) => Some(k),
        Event::StopAll | Event::ResumeAll => None,
    }
}

fn names(effect: Effect<Id>) -> Id {
    match effect {
        Effect::Admit(k)
        | Effect::Refuse(k)
        | Effect::Give(k)
        | Effect::Abort(k)
        | Effect::Stopped(k) => k,
    }
}

/// StopTakesEffect and StoppedInputIsReleased, bounded: a fair caller ends a stopped call in two steps. Its next
/// input is withheld, it releases what it holds and reports the abort, and a stopping session is then stopped.
fn stopped_call_ends(stops: &Stops<Id>, id: Id) -> Result<(), TestCaseError> {
    let mut stops = stops.clone();
    let Some(before) = stops.get(id) else {
        return Ok(());
    };
    if before.run != Run::Stopping && before.call != Call::Aborting {
        return Ok(());
    }
    prop_assert_eq!(
        before.call,
        Call::Aborting,
        "StopTakesEffect: session {} is stopping with nothing to end: {:?}",
        id,
        before
    );
    prop_assert_eq!(
        stops.step(Event::Move(id)),
        vec![Effect::Abort(id)],
        "StoppedInputIsReleased: a stopped call wasn't told to abort, in {:?}",
        before
    );
    let effects = stops.step(Event::Abort(id));
    let after = stops.get(id);
    if before.run == Run::Stopping {
        prop_assert_eq!(
            effects,
            vec![Effect::Stopped(id)],
            "StopTakesEffect: no answer once released, from {:?}",
            before
        );
        prop_assert_eq!(
            after.map(|s| (s.run, s.call)),
            Some((Run::Stopped, Call::Idle)),
            "StopTakesEffect, from {:?}",
            before
        );
    } else {
        prop_assert_eq!(
            effects,
            vec![],
            "a resumed session's abort answered a stop, from {:?}",
            before
        );
        prop_assert_eq!(
            after.map(|s| s.call),
            Some(Call::Idle),
            "StoppedInputIsReleased, from {:?}",
            before
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    #[test]
    fn every_order_of_events_keeps_the_specs_properties(
        picks in proptest::collection::vec((0..KINDS.len(), 0..SLOTS), 0..96)
    ) {
        let mut stops: Stops<Id> = Stops::new();
        let mut world = [Slot::default(); SLOTS];
        let mut next_id: Id = 1;

        for (kind, slot) in picks {
            let target = world[slot].open.unwrap_or(world[slot].last);
            let event = match KINDS[kind] {
                Kind::Connect => {
                    let id = world[slot].open.unwrap_or(next_id);
                    if world[slot].open.is_none() {
                        next_id += 1;
                        world[slot] = Slot { open: Some(id), last: id, held: false, served: false };
                    }
                    Event::Connect(id)
                }
                Kind::Disconnect => Event::Disconnect(target),
                Kind::ToolCall => Event::ToolCall(target),
                Kind::Press => Event::Press(target),
                Kind::Move => Event::Move(target),
                Kind::Release => Event::Release(target),
                Kind::Finish => Event::Finish(target),
                Kind::Abort => Event::Abort(target),
                Kind::Stop => Event::Stop(target),
                Kind::StopAll => Event::StopAll,
                Kind::Resume => Event::Resume(target),
                Kind::ResumeAll => Event::ResumeAll,
            };
            let is_new = matches!(event, Event::Connect(id) if stops.get(id).is_none());

            let before = stops.clone();
            let effects = stops.step(event);

            // The world, as a correct caller makes it.
            match event {
                Event::Disconnect(id) if world[slot].open == Some(id) => {
                    world[slot].open = None;
                    world[slot].held = false;
                }
                Event::Release(id) | Event::Abort(id) if world[slot].open == Some(id) => {
                    world[slot].held = false;
                }
                _ => {}
            }
            for &effect in &effects {
                if let Effect::Admit(id) | Effect::Give(id) = effect {
                    let Some(s) = world.iter_mut().find(|s| s.open == Some(id)) else {
                        return Err(TestCaseError::fail(format!("{effect:?} for a session that isn't open")));
                    };
                    let was = before.get(id);
                    s.served |= was.map(|w| w.run) != Some(Run::Running);
                    if matches!(event, Event::Press(_)) {
                        s.held = true;
                    }
                }
            }

            // Every effect is about the event's own session; only StopAll speaks for several.
            if let Some(id) = about(event) {
                for &effect in &effects {
                    prop_assert_eq!(names(effect), id, "an effect for another session: {:?} gave {:?}", event, effects);
                }
            }
            // Each effect answers the event it may answer.
            for &effect in &effects {
                let fits = match effect {
                    Effect::Admit(_) | Effect::Refuse(_) => matches!(event, Event::ToolCall(_)),
                    Effect::Give(_) => matches!(event, Event::Press(_) | Event::Move(_)),
                    Effect::Abort(_) => matches!(event, Event::Press(_) | Event::Move(_) | Event::Release(_) | Event::Finish(_)),
                    Effect::Stopped(_) => matches!(event, Event::Stop(_) | Event::StopAll | Event::Abort(_)),
                };
                prop_assert!(fits, "an effect that doesn't answer its event: {:?} gave {:?}", event, effects);
            }

            // NoCallServedWhileStopped: once a session's stop reached the daemon, until it is resumed, no call of
            // it is let through and it gives no input; a stopped call gives none even after a resume.
            for &effect in &effects {
                if let Effect::Admit(id) | Effect::Give(id) = effect {
                    let was = before.get(id);
                    prop_assert_eq!(was.map(|w| w.run), Some(Run::Running), "NoCallServedWhileStopped: {:?} for {:?} on {:?}", effect, was, event);
                    prop_assert_ne!(was.map(|w| w.call), Some(Call::Aborting), "NoCallServedWhileStopped: a stopped call gave input on {:?}", event);
                }
            }
            for s in &world {
                prop_assert!(!s.served, "NoCallServedWhileStopped: served while stopped, on {:?}", event);
            }
            // ... and its calls are refused, so the agent is told.
            if let Event::ToolCall(id) = event
                && let Some(was) = before.get(id)
            {
                if was.run != Run::Running {
                    prop_assert_eq!(&effects, &vec![Effect::Refuse(id)], "NoCallServedWhileStopped: a call in {:?} wasn't refused", was);
                    prop_assert_eq!(stops.get(id), Some(was), "a refused call changed its session");
                } else if was.call == Call::Idle {
                    prop_assert_eq!(&effects, &vec![Effect::Admit(id)], "RunningSessionServes: a call in {:?} wasn't admitted", was);
                } else {
                    prop_assert_eq!(&effects, &vec![], "a second call was answered while one is in progress, in {:?}", was);
                }
            }

            for s in &world {
                let Some(id) = s.open else { continue };
                // StoppedScreenStays: a session is known exactly while its screen is up.
                let Some(now) = stops.get(id) else {
                    return Err(TestCaseError::fail(format!("StoppedScreenStays: session {id} forgotten on {event:?} while its screen is up")));
                };
                // NoCallServedWhileStopped, probed: a session that isn't running, or a call that was stopped, gets
                // no input whatever it asks for next, and a stopped session's next call is refused.
                if now.run != Run::Running || now.call == Call::Aborting {
                    for ask in [Event::Press(id), Event::Move(id)] {
                        let answer = stops.clone().step(ask);
                        prop_assert!(!answer.contains(&Effect::Give(id)), "NoCallServedWhileStopped: {:?} gave {:?} in {:?}, after {:?}", ask, answer, now, event);
                    }
                }
                if now.run != Run::Running {
                    prop_assert_eq!(stops.clone().step(Event::ToolCall(id)), vec![Effect::Refuse(id)], "NoCallServedWhileStopped: a call in {:?} isn't refused, after {:?}", now, event);
                }
                // StoppedMeansReleased.
                if now.run == Run::Stopped {
                    prop_assert!(now.call == Call::Idle && !s.held, "StoppedMeansReleased: stopped with {:?}, held {}, on {:?}", now.call, s.held, event);
                }
                // OnlyInputHolds.
                if s.held {
                    prop_assert!(matches!(now.call, Call::Input | Call::Aborting), "OnlyInputHolds: held with {:?}, on {:?}", now.call, event);
                }
                // RunningSessionServes.
                if now.run == Run::Running && now.call == Call::Idle {
                    prop_assert_eq!(stops.clone().step(Event::ToolCall(id)), vec![Effect::Admit(id)], "RunningSessionServes: a call isn't admitted in {:?}, after {:?}", now, event);
                }
                // StopTakesEffect, StoppedInputIsReleased (bounded).
                stopped_call_ends(&stops, id)?;
            }
            for s in &world {
                if s.open.is_none() {
                    prop_assert_eq!(stops.get(s.last), None, "StoppedScreenStays: a gone session is still known after {:?}", event);
                }
            }

            // OnlyDisconnectEndsScreen: only its own Disconnect forgets a session.
            for s in &world {
                let id = s.open.unwrap_or(s.last);
                if before.get(id).is_some() && stops.get(id).is_none() {
                    prop_assert_eq!(event, Event::Disconnect(id), "OnlyDisconnectEndsScreen: session {} forgotten", id);
                }
            }

            // SessionsAreIndependent: a step of one session never changes another.
            if let Some(id) = about(event) {
                for s in &world {
                    let other = s.open.unwrap_or(s.last);
                    if other != id {
                        prop_assert_eq!(stops.get(other), before.get(other), "SessionsAreIndependent: {:?} changed session {}", event, other);
                    }
                }
            }

            // A session becomes stopped only by a stop or an abort, and says so once.
            for s in &world {
                let id = s.open.unwrap_or(s.last);
                let was = before.get(id).map(|w| w.run);
                let now = stops.get(id).map(|n| n.run);
                let became = was.is_some() && was != Some(Run::Stopped) && now == Some(Run::Stopped);
                let said = effects.iter().filter(|e| **e == Effect::Stopped(id)).count();
                prop_assert_eq!(said, usize::from(became), "stop answered wrongly: {:?}: {:?} -> {:?}, effects {:?}", event, was, now, effects);
            }

            // StopAllHoldsForNewAgents: while stop-all is in force a new session starts stopped, and only the
            // two commands for all change the switch.
            match event {
                Event::StopAll => prop_assert!(stops.stop_new(), "StopAllHoldsForNewAgents: stop-all isn't in force after it"),
                Event::ResumeAll => prop_assert!(!stops.stop_new(), "StopAllHoldsForNewAgents: stop-all is in force after resume-all"),
                _ => prop_assert_eq!(stops.stop_new(), before.stop_new(), "StopAllHoldsForNewAgents: {:?} changed stop-all", event),
            }
            if is_new && let Event::Connect(id) = event {
                let run = stops.get(id).map(|n| n.run);
                let expected = if before.stop_new() { Run::Stopped } else { Run::Running };
                prop_assert_eq!(run, Some(expected), "StopAllHoldsForNewAgents: stop-all {} at connect", before.stop_new());
                prop_assert_eq!(stops.get(id).map(|n| n.call), Some(Call::Idle), "a new session has a call");
            }
            // Stop-all covers every open session: none is left running by it.
            if event == Event::StopAll {
                for s in &world {
                    if let Some(id) = s.open {
                        prop_assert_ne!(stops.get(id).map(|n| n.run), Some(Run::Running), "stop-all left session {} running", id);
                    }
                }
            }
            if event == Event::ResumeAll {
                for s in &world {
                    if let Some(id) = s.open {
                        prop_assert_eq!(stops.get(id).map(|n| n.run), Some(Run::Running), "resume-all left session {} stopped", id);
                    }
                }
            }
        }
    }
}
