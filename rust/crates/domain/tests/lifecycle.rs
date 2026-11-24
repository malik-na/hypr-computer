//! The session lifecycle, tested at its seam: `step`, against specs/SessionLifecycle.tla.
#![forbid(unsafe_code)]

use domain::lifecycle::{Effect, Event, SessionState, step};

#[test]
fn an_agent_connecting_starts_a_session_and_spawns_its_screen() {
    assert_eq!(
        step(SessionState::Absent, Event::Connect),
        (SessionState::Starting, Some(Effect::SpawnScreen))
    );
}

#[test]
fn a_screen_that_has_started_is_verified_before_the_agent_gets_it() {
    assert_eq!(
        step(SessionState::Starting, Event::ScreenStarted),
        (SessionState::Verifying, Some(Effect::VerifyScreen))
    );
}

#[test]
fn a_tool_call_is_rejected_unless_the_screen_is_verified_ready() {
    for state in [
        SessionState::Absent,
        SessionState::Starting,
        SessionState::Verifying,
        SessionState::TearingDown,
    ] {
        assert_eq!(
            step(state, Event::ToolCall),
            (state, Some(Effect::RejectToolCall)),
            "in {state:?}"
        );
    }
}

#[test]
fn once_the_screen_passes_its_readiness_check_tool_calls_are_served() {
    let (ready, effect) = step(SessionState::Verifying, Event::ScreenReady);
    assert_eq!((ready, effect), (SessionState::Ready, None));
    assert_eq!(
        step(ready, Event::ToolCall),
        (SessionState::Ready, Some(Effect::ServeToolCall))
    );
}

#[test]
fn the_daemon_tears_down_a_session_whose_agent_disconnects_whatever_its_state() {
    for state in [
        SessionState::Starting,
        SessionState::Verifying,
        SessionState::Ready,
    ] {
        assert_eq!(
            step(state, Event::Disconnect),
            (SessionState::TearingDown, Some(Effect::Teardown)),
            "in {state:?}"
        );
    }
}

#[test]
fn a_session_already_being_torn_down_is_not_torn_down_again() {
    assert_eq!(
        step(SessionState::TearingDown, Event::Disconnect),
        (SessionState::TearingDown, None)
    );
}

#[test]
fn a_screen_that_fails_or_crashes_ends_its_session() {
    let cases = [
        (SessionState::Starting, Event::ScreenFailed),
        (SessionState::Verifying, Event::ScreenFailed),
        (SessionState::Starting, Event::ScreenCrashed),
        (SessionState::Verifying, Event::ScreenCrashed),
        (SessionState::Ready, Event::ScreenCrashed),
    ];
    for (state, event) in cases {
        assert_eq!(
            step(state, event),
            (SessionState::TearingDown, Some(Effect::Teardown)),
            "{event:?} in {state:?}"
        );
    }
}

#[test]
fn a_finished_teardown_drops_the_session_and_closes_the_connection() {
    assert_eq!(
        step(SessionState::TearingDown, Event::TeardownDone),
        (SessionState::Absent, Some(Effect::CloseConnection))
    );
}

#[test]
fn a_screen_that_finishes_starting_during_teardown_changes_nothing() {
    // The teardown already under way stops it: it runs until the screen process is seen gone.
    assert_eq!(
        step(SessionState::TearingDown, Event::ScreenStarted),
        (SessionState::TearingDown, None)
    );
}
