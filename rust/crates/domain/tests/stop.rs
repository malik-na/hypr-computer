//! Stopping agents, tested at its seam: `domain::stop`, against specs/Stop.tla.
//!
//! # The API these tests fix
//!
//! `rust/crates/domain/src/stop.rs`, declared `pub mod stop;` in `lib.rs`. It holds what the daemon believes about
//! stopping: the spec's `run`, `call` and `stopNew`. The spec's `session` and `screen` are the lifecycle's, and
//! `held` and `moved` are the world: what the screen holds down is the caller's business, reported as events.
//!
//! ```ignore
//! /// Whether a session serves. The spec's `run`.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Run { Running, Stopping, Stopped }
//!
//! /// The session's tool call in progress. The spec's `call`.
//! /// `Admitted`: let through, holding nothing.  `Input`: holding a button or key down.
//! /// `Aborting`: stopped while admitted or holding; it gives no more input, only its release.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Call { Idle, Admitted, Input, Aborting }
//!
//! /// One session's stop state.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub struct SessionStop { pub run: Run, pub call: Call }
//!
//! /// Something that happened, or is asked for, in session `K` (the daemon uses `SessionId`).
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Event<K> {
//!     Connect(K), Disconnect(K),
//!     ToolCall(K), Press(K), Move(K), Release(K), Finish(K), Abort(K),
//!     Stop(K), StopAll, Resume(K), ResumeAll,
//! }
//!
//! /// What the caller must do because of an event.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Effect<K> { Admit(K), Refuse(K), Give(K), Abort(K), Stopped(K) }
//!
//! /// The stop state of every connected session, and the stop-all switch (the spec's `stopNew`).
//! #[derive(Clone, Debug)]          // PartialEq, Eq and Default are welcome, not required
//! pub struct Stops<K> { /* private */ }
//!
//! impl<K: Copy + Ord> Stops<K> {
//!     /// No session, stop-all off.
//!     pub fn new() -> Self;
//!     /// Applies one event, and says what to do about it. Pure: no I/O, no clock.
//!     pub fn step(&mut self, event: Event<K>) -> Vec<Effect<K>>;
//!     /// The session's stop state; `None` before its `Connect` and after its `Disconnect`.
//!     pub fn get(&self, id: K) -> Option<SessionStop>;
//!     /// Whether `hypr-computer stop` (all) is in force: a session that connects starts stopped.
//!     pub fn stop_new(&self) -> bool;
//! }
//! ```
//!
//! `step`, for a session `k` whose state is `s` before the event. Every case not listed changes nothing and
//! returns no effect: an event for an unknown `k`, a `Connect` of a known one, `Press`/`Move`/`Release`/`Finish`
//! while `Idle`, `Release` while `Admitted`, `Finish` while `Input`, `Abort` unless `Aborting`.
//!
//! | Event | When | Then | Effects |
//! | --- | --- | --- | --- |
//! | `Connect(k)` | `k` unknown | `Stopped` if stop-all is in force, else `Running`; `Idle` | none |
//! | `Disconnect(k)` | `k` known | `k` forgotten | none |
//! | `ToolCall(k)` | `Running`, `Idle` | `Admitted` | `Admit(k)` |
//! | `ToolCall(k)` | `Stopping` or `Stopped` | unchanged | `Refuse(k)`, whatever the call |
//! | `ToolCall(k)` | `Running`, call not `Idle` | unchanged | none: a call is in progress |
//! | `Press(k)` | `Admitted` or `Input` | `Input` | `Give(k)` |
//! | `Move(k)` | `Admitted` or `Input` | unchanged | `Give(k)` |
//! | `Release(k)` | `Input` | `Admitted` | none |
//! | `Finish(k)` | `Admitted` | `Idle` | none |
//! | `Press`, `Move`, `Release`, `Finish` | `Aborting` | unchanged | `Abort(k)` |
//! | `Abort(k)` | `Aborting` | `Idle`; `Stopping` becomes `Stopped` | `Stopped(k)` if it became `Stopped` |
//! | `Stop(k)` | `Running` | `Stopping`, `Admitted`/`Input` become `Aborting`; then `Stopped` at once if `Idle` | `Stopped(k)` if it became `Stopped` |
//! | `Stop(k)` | `Stopping` or `Stopped` | unchanged | none |
//! | `StopAll` | always | stop-all on; every `Running` session as by `Stop` | `Stopped(k)` for each that became `Stopped`, any order |
//! | `Resume(k)` | not `Running` | `Running`; the call is unchanged (an `Aborting` call still ends with `Abort`) | none |
//! | `ResumeAll` | always | stop-all off; every session `Running` | none |
//!
//! So a session is never `Stopping` with nothing in progress: the spec's `Stopped` action happens in the same step
//! as the `Stop` or the `Abort` that ends the last input. `Admitted` and `Input` imply `Running`.
//!
//! What the caller must do, because `step` can't check it:
//! - **One `Stops` per daemon, owned by one task, stepped one event at a time.** `Connect` a fresh id when a
//!   session is created (the lifecycle's `Connect`, so a stop-all covers sessions whose screen is still starting)
//!   and `Disconnect` it when the session is gone. Ids aren't reused.
//! - **Every tool call steps `ToolCall` first.** On `Admit(k)` it runs; on `Refuse(k)` the agent gets "Stopped by
//!   the user" and the screen is left alone. With no effect, the session's previous call is still in progress:
//!   the new call waits, and steps `ToolCall` again once that call has stepped its `Finish` or `Abort`.
//! - **Every input a call gives the screen is asked for first,** and given only on `Give(k)`: `Press` for one
//!   that leaves a button or key held down, `Move` for any other input that isn't a release (a pointer motion, a
//!   scroll, a move with the button held). A tap is a `Press` and a `Release`.
//! - **`Release(k)` is stepped after the call has released everything it held.** Releasing never waits for
//!   permission. `Finish(k)` is stepped when the call is done and holds nothing; the agent then gets its result.
//! - **On `Abort(k)`** the call gives no more input: it releases everything it holds, then steps `Abort(k)`, and
//!   the agent gets "Stopped by the user" instead of the call's result.
//! - **`Stopped(k)`:** nothing of `k`'s is in progress or held. A `hypr-computer stop` waiting on `k` answers then,
//!   or when `k` is resumed or disconnects first (`get(k)` says which). One stopped before it was asked answers
//!   at once.
#![forbid(unsafe_code)]

use domain::stop::{Call, Effect, Event, Run, SessionStop, Stops};

type Id = u32;

fn connected(ids: &[Id]) -> Stops<Id> {
    let mut stops = Stops::new();
    for &id in ids {
        assert_eq!(stops.step(Event::Connect(id)), vec![]);
    }
    stops
}

fn state(stops: &Stops<Id>, id: Id) -> SessionStop {
    let state = stops.get(id);
    assert!(state.is_some(), "session {id} is gone");
    state.unwrap_or(RUNNING_IDLE)
}

fn sorted(mut effects: Vec<Effect<Id>>) -> Vec<Effect<Id>> {
    effects.sort_by_key(|e| format!("{e:?}"));
    effects
}

const RUNNING_IDLE: SessionStop = SessionStop {
    run: Run::Running,
    call: Call::Idle,
};

const STOPPED_IDLE: SessionStop = SessionStop {
    run: Run::Stopped,
    call: Call::Idle,
};

#[test]
fn an_agent_that_connects_runs_and_its_calls_are_let_through() {
    let mut stops = connected(&[1]);
    assert_eq!(state(&stops, 1), RUNNING_IDLE);
    assert!(!stops.stop_new());

    assert_eq!(stops.step(Event::ToolCall(1)), vec![Effect::Admit(1)]);
    assert_eq!(state(&stops, 1).call, Call::Admitted);
    assert_eq!(stops.step(Event::Finish(1)), vec![]);
    assert_eq!(state(&stops, 1), RUNNING_IDLE);
}

#[test]
fn a_drag_while_running_gives_every_input() {
    let mut stops = connected(&[1]);
    assert_eq!(stops.step(Event::ToolCall(1)), vec![Effect::Admit(1)]);
    assert_eq!(stops.step(Event::Press(1)), vec![Effect::Give(1)]);
    assert_eq!(state(&stops, 1).call, Call::Input);
    assert_eq!(stops.step(Event::Move(1)), vec![Effect::Give(1)]);
    assert_eq!(stops.step(Event::Move(1)), vec![Effect::Give(1)]);
    assert_eq!(stops.step(Event::Release(1)), vec![]);
    assert_eq!(state(&stops, 1).call, Call::Admitted);
    assert_eq!(stops.step(Event::Finish(1)), vec![]);
    assert_eq!(state(&stops, 1), RUNNING_IDLE);
}

#[test]
fn a_call_can_give_several_inputs_before_it_finishes() {
    // Typing "ab": two taps, then done. A pointer move holds nothing.
    let mut stops = connected(&[1]);
    stops.step(Event::ToolCall(1));
    for _ in 0..2 {
        assert_eq!(stops.step(Event::Press(1)), vec![Effect::Give(1)]);
        assert_eq!(stops.step(Event::Release(1)), vec![]);
    }
    assert_eq!(stops.step(Event::Move(1)), vec![Effect::Give(1)]);
    assert_eq!(state(&stops, 1).call, Call::Admitted);
    assert_eq!(stops.step(Event::Finish(1)), vec![]);
    assert_eq!(state(&stops, 1), RUNNING_IDLE);
}

#[test]
fn stopping_an_idle_agent_takes_effect_at_once_and_refuses_its_calls() {
    let mut stops = connected(&[1]);
    assert_eq!(stops.step(Event::Stop(1)), vec![Effect::Stopped(1)]);
    assert_eq!(state(&stops, 1), STOPPED_IDLE);

    for _ in 0..3 {
        assert_eq!(stops.step(Event::ToolCall(1)), vec![Effect::Refuse(1)]);
        assert_eq!(state(&stops, 1), STOPPED_IDLE);
    }
}

#[test]
fn stopping_an_agent_again_changes_nothing() {
    let mut stops = connected(&[1]);
    stops.step(Event::Stop(1));
    let before = stops.clone();
    assert_eq!(stops.step(Event::Stop(1)), vec![]);
    assert_eq!(stops.step(Event::StopAll), vec![]);
    assert_eq!(stops.get(1), before.get(1));
}

/// TLC's counterexample for CheckStopOnlyAtStart: a call is admitted, the user runs `hypr-computer stop`, and the
/// call presses anyway. Here the press is refused, and the stop answers once the call is over.
#[test]
fn a_drag_admitted_before_stop_all_gives_no_input_after_it() {
    let mut stops = connected(&[1]);
    assert_eq!(stops.step(Event::ToolCall(1)), vec![Effect::Admit(1)]);

    assert_eq!(
        stops.step(Event::StopAll),
        vec![],
        "stop answered with a call in progress"
    );
    assert_eq!(
        state(&stops, 1),
        SessionStop {
            run: Run::Stopping,
            call: Call::Aborting
        }
    );

    assert_eq!(stops.step(Event::Press(1)), vec![Effect::Abort(1)]);
    assert_eq!(stops.step(Event::Move(1)), vec![Effect::Abort(1)]);
    assert_eq!(
        state(&stops, 1).run,
        Run::Stopping,
        "stopped before the call was over"
    );

    assert_eq!(stops.step(Event::Abort(1)), vec![Effect::Stopped(1)]);
    assert_eq!(state(&stops, 1), STOPPED_IDLE);
}

#[test]
fn a_drag_holding_its_button_when_stopped_only_releases_it() {
    let mut stops = connected(&[1]);
    stops.step(Event::ToolCall(1));
    assert_eq!(stops.step(Event::Press(1)), vec![Effect::Give(1)]);
    assert_eq!(stops.step(Event::Move(1)), vec![Effect::Give(1)]);

    assert_eq!(stops.step(Event::Stop(1)), vec![]);
    assert_eq!(
        state(&stops, 1),
        SessionStop {
            run: Run::Stopping,
            call: Call::Aborting
        }
    );

    // The drag's next move is withheld, and so is anything else it asks for: it must release and stop.
    assert_eq!(stops.step(Event::Move(1)), vec![Effect::Abort(1)]);
    assert_eq!(stops.step(Event::Press(1)), vec![Effect::Abort(1)]);
    // It released its button, as its abort or as its own last step: the stop still waits for the abort.
    assert_eq!(stops.step(Event::Release(1)), vec![Effect::Abort(1)]);
    assert_eq!(stops.step(Event::Finish(1)), vec![Effect::Abort(1)]);
    assert_eq!(state(&stops, 1).run, Run::Stopping);

    assert_eq!(stops.step(Event::Abort(1)), vec![Effect::Stopped(1)]);
    assert_eq!(state(&stops, 1), STOPPED_IDLE);
}

#[test]
fn a_screenshot_in_progress_when_stopped_is_told_stopped() {
    let mut stops = connected(&[1]);
    stops.step(Event::ToolCall(1));
    assert_eq!(stops.step(Event::Stop(1)), vec![]);
    assert_eq!(stops.step(Event::Finish(1)), vec![Effect::Abort(1)]);
    assert_eq!(stops.step(Event::Abort(1)), vec![Effect::Stopped(1)]);
    assert_eq!(state(&stops, 1), STOPPED_IDLE);
}

#[test]
fn a_stopping_agent_refuses_new_calls_before_its_stop_is_done() {
    let mut stops = connected(&[1]);
    stops.step(Event::ToolCall(1));
    stops.step(Event::Press(1));
    stops.step(Event::Stop(1));
    assert_eq!(stops.step(Event::ToolCall(1)), vec![Effect::Refuse(1)]);
    assert_eq!(state(&stops, 1).call, Call::Aborting);
}

#[test]
fn stop_all_stops_every_agent_and_answers_for_the_idle_ones_at_once() {
    let mut stops = connected(&[1, 2, 3]);
    stops.step(Event::ToolCall(2));
    stops.step(Event::Press(2));

    assert_eq!(
        sorted(stops.step(Event::StopAll)),
        vec![Effect::Stopped(1), Effect::Stopped(3)]
    );
    assert!(stops.stop_new());
    assert_eq!(state(&stops, 1), STOPPED_IDLE);
    assert_eq!(state(&stops, 3), STOPPED_IDLE);
    assert_eq!(
        state(&stops, 2),
        SessionStop {
            run: Run::Stopping,
            call: Call::Aborting
        }
    );
    for id in [1, 2, 3] {
        assert_eq!(stops.step(Event::ToolCall(id)), vec![Effect::Refuse(id)]);
    }

    assert_eq!(stops.step(Event::Move(2)), vec![Effect::Abort(2)]);
    assert_eq!(stops.step(Event::Abort(2)), vec![Effect::Stopped(2)]);
    assert_eq!(state(&stops, 2), STOPPED_IDLE);
}

#[test]
fn stop_all_holds_for_agents_that_connect_later_until_resume_all() {
    let mut stops = connected(&[]);
    stops.step(Event::StopAll);

    assert_eq!(stops.step(Event::Connect(7)), vec![]);
    assert_eq!(state(&stops, 7), STOPPED_IDLE);
    assert_eq!(stops.step(Event::ToolCall(7)), vec![Effect::Refuse(7)]);

    // Reconnecting doesn't get an agent out of a stop.
    stops.step(Event::Disconnect(7));
    stops.step(Event::Connect(8));
    assert_eq!(state(&stops, 8), STOPPED_IDLE);

    assert_eq!(stops.step(Event::ResumeAll), vec![]);
    assert!(!stops.stop_new());
    assert_eq!(state(&stops, 8), RUNNING_IDLE);
    stops.step(Event::Connect(9));
    assert_eq!(state(&stops, 9), RUNNING_IDLE);
    assert_eq!(stops.step(Event::ToolCall(9)), vec![Effect::Admit(9)]);
}

#[test]
fn stopping_one_agent_leaves_the_others_alone() {
    let mut stops = connected(&[1, 2, 3]);
    stops.step(Event::ToolCall(2));
    stops.step(Event::Press(2));
    let before = stops.clone();

    assert_eq!(stops.step(Event::Stop(1)), vec![Effect::Stopped(1)]);
    assert!(!stops.stop_new(), "stopping one agent stops new ones");
    assert_eq!(stops.get(2), before.get(2));
    assert_eq!(stops.get(3), before.get(3));
    assert_eq!(stops.step(Event::Move(2)), vec![Effect::Give(2)]);
    assert_eq!(stops.step(Event::ToolCall(3)), vec![Effect::Admit(3)]);

    // An agent that connects now runs.
    stops.step(Event::Connect(4));
    assert_eq!(state(&stops, 4), RUNNING_IDLE);
}

#[test]
fn resuming_one_agent_lets_it_serve_and_leaves_the_others_stopped() {
    let mut stops = connected(&[1, 2]);
    stops.step(Event::StopAll);

    assert_eq!(stops.step(Event::Resume(1)), vec![]);
    assert_eq!(state(&stops, 1), RUNNING_IDLE);
    assert_eq!(state(&stops, 2), STOPPED_IDLE);
    assert!(stops.stop_new(), "resuming one agent ended stop-all");
    assert_eq!(stops.step(Event::ToolCall(1)), vec![Effect::Admit(1)]);
    assert_eq!(stops.step(Event::ToolCall(2)), vec![Effect::Refuse(2)]);

    // Stop-all is still in force for new agents.
    stops.step(Event::Connect(3));
    assert_eq!(state(&stops, 3), STOPPED_IDLE);
}

#[test]
fn resuming_a_running_agent_changes_nothing() {
    let mut stops = connected(&[1]);
    stops.step(Event::ToolCall(1));
    stops.step(Event::Press(1));
    let before = stops.clone();
    assert_eq!(stops.step(Event::Resume(1)), vec![]);
    assert_eq!(stops.get(1), before.get(1));
}

#[test]
fn a_call_stopped_and_then_resumed_still_ends_with_its_release() {
    let mut stops = connected(&[1]);
    stops.step(Event::ToolCall(1));
    stops.step(Event::Press(1));
    stops.step(Event::Stop(1));

    assert_eq!(stops.step(Event::Resume(1)), vec![]);
    assert_eq!(
        state(&stops, 1),
        SessionStop {
            run: Run::Running,
            call: Call::Aborting
        }
    );
    // The stopped drag doesn't carry on: it may only release.
    assert_eq!(stops.step(Event::Move(1)), vec![Effect::Abort(1)]);
    // The agent's next call waits for it.
    assert_eq!(stops.step(Event::ToolCall(1)), vec![]);

    assert_eq!(
        stops.step(Event::Abort(1)),
        vec![],
        "a resumed session reported stopped"
    );
    assert_eq!(state(&stops, 1), RUNNING_IDLE);
    assert_eq!(stops.step(Event::ToolCall(1)), vec![Effect::Admit(1)]);
}

#[test]
fn a_running_agent_has_one_call_at_a_time() {
    let mut stops = connected(&[1]);
    stops.step(Event::ToolCall(1));
    assert_eq!(stops.step(Event::ToolCall(1)), vec![]);
    stops.step(Event::Press(1));
    assert_eq!(stops.step(Event::ToolCall(1)), vec![]);
    assert_eq!(state(&stops, 1).call, Call::Input);
}

#[test]
fn a_stopped_agent_keeps_its_session_until_it_disconnects() {
    let mut stops = connected(&[1]);
    stops.step(Event::ToolCall(1));
    stops.step(Event::Press(1));
    stops.step(Event::StopAll);
    stops.step(Event::Abort(1));
    stops.step(Event::Resume(1));
    stops.step(Event::Stop(1));
    stops.step(Event::ResumeAll);
    assert!(stops.get(1).is_some(), "stop or resume ended the session");

    assert_eq!(stops.step(Event::Disconnect(1)), vec![]);
    assert_eq!(stops.get(1), None);
}

#[test]
fn an_agent_that_disconnects_mid_stop_is_forgotten() {
    let mut stops = connected(&[1]);
    stops.step(Event::ToolCall(1));
    stops.step(Event::Press(1));
    stops.step(Event::Stop(1));
    assert_eq!(stops.step(Event::Disconnect(1)), vec![]);
    assert_eq!(stops.get(1), None);
    // A late event from the gone session does nothing, and a new session doesn't inherit its stop.
    assert_eq!(stops.step(Event::Abort(1)), vec![]);
    assert_eq!(stops.step(Event::ToolCall(1)), vec![]);
    assert_eq!(stops.get(1), None);
    stops.step(Event::Connect(2));
    assert_eq!(state(&stops, 2), RUNNING_IDLE);
}

#[test]
fn events_for_an_unknown_agent_change_nothing() {
    let mut stops = connected(&[1]);
    stops.step(Event::StopAll);
    let before = stops.clone();
    for event in [
        Event::Disconnect(9),
        Event::ToolCall(9),
        Event::Press(9),
        Event::Move(9),
        Event::Release(9),
        Event::Finish(9),
        Event::Abort(9),
        Event::Stop(9),
        Event::Resume(9),
    ] {
        assert_eq!(stops.step(event), vec![], "{event:?}");
    }
    assert_eq!(stops.get(9), None);
    assert_eq!(stops.get(1), before.get(1));
    assert_eq!(stops.stop_new(), before.stop_new());
}

#[test]
fn connecting_a_known_agent_again_changes_nothing() {
    let mut stops = connected(&[1]);
    stops.step(Event::ToolCall(1));
    stops.step(Event::StopAll);
    let before = stops.get(1);
    assert_eq!(stops.step(Event::Connect(1)), vec![]);
    assert_eq!(stops.get(1), before);
}

#[test]
fn inputs_reported_out_of_order_change_nothing() {
    let mut stops = connected(&[1]);
    // No call in progress.
    for event in [
        Event::Press(1),
        Event::Move(1),
        Event::Release(1),
        Event::Finish(1),
        Event::Abort(1),
    ] {
        assert_eq!(stops.step(event), vec![], "{event:?} with no call");
        assert_eq!(state(&stops, 1), RUNNING_IDLE);
    }
    // Admitted, holding nothing: nothing to release, nothing to abort.
    stops.step(Event::ToolCall(1));
    assert_eq!(stops.step(Event::Release(1)), vec![]);
    assert_eq!(stops.step(Event::Abort(1)), vec![]);
    assert_eq!(state(&stops, 1).call, Call::Admitted);
    // Holding: it can't finish before it releases.
    stops.step(Event::Press(1));
    assert_eq!(stops.step(Event::Finish(1)), vec![]);
    assert_eq!(stops.step(Event::Abort(1)), vec![]);
    assert_eq!(state(&stops, 1).call, Call::Input);
}
