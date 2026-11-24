//! The session lifecycle against the invariants of specs/SessionLifecycle.tla, over random sequences of events
//! arriving in any order, including ones the spec doesn't allow in a state.
#![forbid(unsafe_code)]

use domain::lifecycle::{Effect, Event, SessionState, step};
use proptest::prelude::*;

const EVENTS: [Event; 8] = [
    Event::Connect,
    Event::ScreenStarted,
    Event::ScreenReady,
    Event::ScreenFailed,
    Event::ScreenCrashed,
    Event::ToolCall,
    Event::Disconnect,
    Event::TeardownDone,
];

/// The session's history since it last connected, as far as the invariants need it.
#[derive(Default)]
struct Since {
    verified: bool,
    teardowns: u32,
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    #[test]
    fn every_order_of_events_keeps_the_specs_invariants(picks in proptest::collection::vec(0..EVENTS.len(), 0..64)) {
        let mut state = SessionState::Absent;
        let mut since = Since::default();
        for pick in picks {
            let event = EVENTS[pick];
            let (next, effect) = step(state, event);

            // ToolCallOnlyOnReadyScreen: a tool call is served only on a screen the readiness check passed.
            if effect == Some(Effect::ServeToolCall) {
                prop_assert!(since.verified, "served a tool call on a screen that wasn't verified ready");
            }
            // A screen is ready only after it was verified.
            if next == SessionState::Ready && state != SessionState::Ready {
                prop_assert_eq!((state, event), (SessionState::Verifying, Event::ScreenReady));
                since.verified = true;
            }
            // TeardownAtMostOnce.
            if effect == Some(Effect::Teardown) {
                since.teardowns += 1;
                prop_assert!(since.teardowns <= 1, "teardown started twice for one session");
                prop_assert_eq!(next, SessionState::TearingDown);
            }
            // The daemon starts the teardown itself the moment the agent is gone (not the dying process).
            if event == Event::Disconnect && matches!(state, SessionState::Starting | SessionState::Verifying | SessionState::Ready) {
                prop_assert_eq!(effect, Some(Effect::Teardown));
            }
            // NoScreenWithoutSession: the session is dropped only once its teardown is done, and the
            // connection is closed with it.
            if next == SessionState::Absent && state != SessionState::Absent {
                prop_assert_eq!((state, event, effect), (SessionState::TearingDown, Event::TeardownDone, Some(Effect::CloseConnection)));
            }
            // A session that is being torn down serves nothing and never comes back to life.
            if state == SessionState::TearingDown {
                prop_assert!(matches!(next, SessionState::TearingDown | SessionState::Absent));
                prop_assert_ne!(effect, Some(Effect::ServeToolCall));
            }
            // Every tool call gets an answer: served on a ready screen, refused otherwise.
            if event == Event::ToolCall && state != SessionState::Ready {
                prop_assert_eq!(effect, Some(Effect::RejectToolCall));
            }
            // An event the spec doesn't allow changes nothing; at most the caller gets a refusal.
            if next == state && effect.is_some() && !(state == SessionState::Ready && event == Event::ToolCall) {
                prop_assert_eq!((event, effect), (Event::ToolCall, Some(Effect::RejectToolCall)));
            }
            // DaemonNeverStuck: every state has an event that moves it on.
            prop_assert!(EVENTS.iter().any(|e| step(next, *e).0 != next));

            if next == SessionState::Absent {
                since = Since::default();
            }
            state = next;
        }
    }
}
