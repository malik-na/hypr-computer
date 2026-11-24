//! Taking over an agent's screen, tested at its seam: `domain::takeover`, against specs/TakeOver.tla.
//!
//! # The API these tests fix
//!
//! `rust/crates/domain/src/takeover.rs`, declared `pub mod takeover;` in `lib.rs`. It holds what the daemon
//! believes about take-overs: the spec's `control` and `call`, and the user's last key, for Escape twice. The
//! spec's `session` is the lifecycle's, and `clip` is the world: the clipboards are joined from a `TakenOver(k)`
//! effect to the next `Given(k)`.
//!
//! ```ignore
//! /// Who drives the screen. The spec's `control`.
//! /// `Taking`: the user clicked in while a call was running; they get the screen when it ends.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Control { Agent, Taking, User }
//!
//! /// The agent's tool call. The spec's `call`.
//! /// `Waiting`: arrived while the screen was taken over; no answer, no input yet.
//! /// `Running`: let through, it may give input until it reports `Finish`.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Call { Idle, Waiting, Running }
//!
//! /// One session's take-over state.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub struct SessionTakeOver { pub control: Control, pub call: Call }
//!
//! /// A key the user pressed in a view, as far as handing back cares.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Key { Escape, Other }
//!
//! /// Something that happened, or is asked for, in session `K` (the daemon uses `SessionId`).
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Event<K> {
//!     Connect(K), Disconnect(K),
//!     /// The agent's tool call reaches the guard; the admitted (or resumed) call is done.
//!     ToolCall(K), Finish(K),
//!     /// The user, in `K`'s live view: presses a pointer button; any other pointer input (motion, a
//!     /// button's release, scroll); presses a key.
//!     Click(K), Pointer(K), Key(K, Key),
//!     /// `hypr-computer take <agent>`; `hypr-computer give`.
//!     Take(K), Give,
//! }
//!
//! /// What the caller must do because of an event.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Effect<K> {
//!     /// Run the agent's call now; hold it, unanswered; run the held call now.
//!     Admit(K), Wait(K), Resume(K),
//!     /// Give the user's input to `K`'s screen.
//!     Forward(K),
//!     /// The user has `K`'s screen: show the banner, join the clipboards.
//!     TakenOver(K),
//!     /// `K`'s screen is handed back: banner off, clipboards apart, the user's held buttons and keys released.
//!     Given(K),
//! }
//!
//! /// The take-over state of every connected session.
//! #[derive(Clone, Debug)]          // PartialEq, Eq and Default are welcome, not required
//! pub struct TakeOvers<K> { /* private */ }
//!
//! impl<K: Copy + Ord> TakeOvers<K> {
//!     /// No session.
//!     pub fn new() -> Self;
//!     /// Applies one event, and says what to do about it. Pure: no I/O, no clock.
//!     pub fn step(&mut self, event: Event<K>) -> Vec<Effect<K>>;
//!     /// The session's take-over state; `None` before its `Connect` and after its `Disconnect`.
//!     pub fn get(&self, id: K) -> Option<SessionTakeOver>;
//!     /// The session taken over (`Taking` or `User`), if any. There is at most one.
//!     pub fn taken(&self) -> Option<K>;
//! }
//! ```
//!
//! `step`, for a session `k` whose state is `s` before the event. Every case not listed changes nothing and
//! returns no effect: an event for an unknown `k`, a `Connect` of a known one, `Finish` unless `Running`, `Give`
//! with nothing taken over, `Take` of a session already taken over.
//!
//! | Event | When | Then | Effects |
//! | --- | --- | --- | --- |
//! | `Connect(k)` | `k` unknown | `Agent`, `Idle` | none |
//! | `Disconnect(k)` | `k` known | `k` forgotten; a waiting call is dropped, not resumed | `Given(k)` if `k` was `Taking` or `User` |
//! | `ToolCall(k)` | `Idle`, `Agent` | `Running` | `Admit(k)` |
//! | `ToolCall(k)` | `Idle`, `Taking` or `User` | `Waiting` | `Wait(k)` |
//! | `ToolCall(k)` | not `Idle` | unchanged | none: a call is in progress |
//! | `Finish(k)` | `Running`, `Agent` | `Idle` | none |
//! | `Finish(k)` | `Running`, `Taking` | `Idle`, `User` | `TakenOver(k)` |
//! | `Click(k)`, `Take(k)` | `Agent`, nothing taken over | `User`, or `Taking` if `Running`; the click is given to no one | `TakenOver(k)`, or none if `Taking` |
//! | `Click(k)`, `Take(k)` | `Agent`, `j` taken over | `j` handed back as by `Give`, then `k` as above | `j`'s, then `k`'s |
//! | `Click(k)` | `User` | unchanged | `Forward(k)` |
//! | `Pointer(k)` | `User` | unchanged | `Forward(k)` |
//! | `Key(k, Other)` | `User` | unchanged | `Forward(k)` |
//! | `Key(k, Escape)` | `User`, the user's last key or click since the take-over wasn't Escape | unchanged | `Forward(k)` |
//! | `Key(k, Escape)` | `User`, the user's last key or click since the take-over was Escape | `Agent`; `Waiting` becomes `Running` | `Given(k)`, `Resume(k)` if it was `Waiting` |
//! | `Click`, `Pointer`, `Key` | `Agent` (watch-only, other than a click) or `Taking` | unchanged | none: the input is dropped |
//! | `Give` | `j` is `Taking` or `User` | `Agent`; `Waiting` becomes `Running` | `Given(j)`, `Resume(j)` if it was `Waiting` |
//!
//! Effects of one step may come in any order. So the user's input reaches a screen only while they have it and
//! no call of the agent's is running there, a call waits only while the screen is taken over, and a waiting call
//! is resumed exactly once, in the step that hands back. A `Pointer` (a motion, a release, a scroll) neither
//! counts as a key nor breaks an Escape pair; a click or another key does.
//!
//! What the caller must do, because `step` can't check it:
//! - **One `TakeOvers` per daemon, owned by one task, stepped one event at a time.** `Connect` a fresh id when a
//!   session is created and `Disconnect` it when the session is gone. Ids aren't reused.
//! - **Every tool call steps `ToolCall` first,** screenshots included. On `Admit(k)` it runs. On `Wait(k)` it is
//!   held: no answer to the agent, no input to the screen. On `Resume(k)` the held call runs, and its result tells
//!   the agent the user took over its screen and handed it back. With no effect, the session's previous call is
//!   still in progress: the new one waits for it, and steps `ToolCall` again once that one has stepped `Finish`.
//! - **`Finish(k)` is stepped when a running call has given all its input,** before the agent gets its result.
//! - **The view reports the user's input:** a pointer button pressed is `Click(k)`, a key pressed is
//!   `Key(k, ..)`, anything else (motion, scroll, a release) is `Pointer(k)`. It gives the screen exactly the
//!   inputs answered with `Forward(k)`, at the same place in the screen as in the view, and a release only if it
//!   gave the press. The click that takes over and the Escape that hands back are given to no one.
//! - **`TakenOver(k)`:** show the banner on `k`'s view and join the clipboards. **`Given(k)`:** take the banner
//!   down, part the clipboards, and release anything of the user's still held on `k`'s screen.
//! - **`Take` and `Give`** are `hypr-computer take <agent>` and `hypr-computer give`. With no effect, `give` had
//!   nothing to hand back, and says so.
#![forbid(unsafe_code)]

use domain::takeover::{Call, Control, Effect, Event, Key, SessionTakeOver, TakeOvers};

type Id = u32;

fn connected(ids: &[Id]) -> TakeOvers<Id> {
    let mut t = TakeOvers::new();
    for &id in ids {
        assert_eq!(t.step(Event::Connect(id)), vec![]);
    }
    t
}

fn state(t: &TakeOvers<Id>, id: Id) -> SessionTakeOver {
    let state = t.get(id);
    assert!(state.is_some(), "session {id} is gone");
    state.unwrap_or(AGENT_IDLE)
}

fn sorted(mut effects: Vec<Effect<Id>>) -> Vec<Effect<Id>> {
    effects.sort_by_key(|e| format!("{e:?}"));
    effects
}

const AGENT_IDLE: SessionTakeOver = SessionTakeOver {
    control: Control::Agent,
    call: Call::Idle,
};

const USER_IDLE: SessionTakeOver = SessionTakeOver {
    control: Control::User,
    call: Call::Idle,
};

/// Every kind of input the user can give in a view.
fn user_inputs(id: Id) -> [Event<Id>; 4] {
    [
        Event::Pointer(id),
        Event::Key(id, Key::Other),
        Event::Key(id, Key::Escape),
        Event::Click(id),
    ]
}

#[test]
fn an_agent_that_connects_drives_its_own_screen() {
    let mut t = connected(&[1]);
    assert_eq!(state(&t, 1), AGENT_IDLE);
    assert_eq!(t.taken(), None);

    assert_eq!(t.step(Event::ToolCall(1)), vec![Effect::Admit(1)]);
    assert_eq!(state(&t, 1).call, Call::Running);
    assert_eq!(t.step(Event::Finish(1)), vec![]);
    assert_eq!(state(&t, 1), AGENT_IDLE);
}

#[test]
fn the_view_is_watch_only_until_the_user_clicks() {
    let mut t = connected(&[1]);
    for event in [
        Event::Pointer(1),
        Event::Key(1, Key::Other),
        Event::Key(1, Key::Escape),
        Event::Key(1, Key::Escape),
        Event::Pointer(1),
    ] {
        assert_eq!(t.step(event), vec![], "{event:?} on a watch-only view");
    }
    assert_eq!(state(&t, 1), AGENT_IDLE);
    assert_eq!(t.taken(), None);
}

#[test]
fn a_click_into_the_view_takes_over_and_is_given_to_no_one() {
    let mut t = connected(&[1]);
    assert_eq!(t.step(Event::Click(1)), vec![Effect::TakenOver(1)]);
    assert_eq!(state(&t, 1), USER_IDLE);
    assert_eq!(t.taken(), Some(1));
}

#[test]
fn once_taken_over_the_users_input_goes_to_the_screen() {
    let mut t = connected(&[1]);
    t.step(Event::Click(1));
    for event in [
        Event::Pointer(1),
        Event::Click(1),
        Event::Pointer(1),
        Event::Key(1, Key::Other),
        Event::Key(1, Key::Escape),
    ] {
        assert_eq!(t.step(event), vec![Effect::Forward(1)], "{event:?}");
        assert_eq!(state(&t, 1), USER_IDLE);
    }
}

#[test]
fn hypr_computer_take_takes_over_like_a_click() {
    let mut t = connected(&[1]);
    assert_eq!(t.step(Event::Take(1)), vec![Effect::TakenOver(1)]);
    assert_eq!(state(&t, 1), USER_IDLE);
    // Taking it again changes nothing.
    assert_eq!(t.step(Event::Take(1)), vec![]);
    assert_eq!(state(&t, 1), USER_IDLE);
}

#[test]
fn the_agents_calls_wait_while_the_user_has_the_screen() {
    let mut t = connected(&[1]);
    t.step(Event::Click(1));
    assert_eq!(t.step(Event::ToolCall(1)), vec![Effect::Wait(1)]);
    assert_eq!(
        state(&t, 1),
        SessionTakeOver {
            control: Control::User,
            call: Call::Waiting
        }
    );
    // The agent's next call waits behind it, unanswered.
    assert_eq!(t.step(Event::ToolCall(1)), vec![]);
    // The user's input still goes to the screen.
    assert_eq!(t.step(Event::Click(1)), vec![Effect::Forward(1)]);
}

#[test]
fn give_hands_back_and_the_waiting_call_runs_once() {
    let mut t = connected(&[1]);
    t.step(Event::Click(1));
    t.step(Event::ToolCall(1));

    assert_eq!(
        sorted(t.step(Event::Give)),
        sorted(vec![Effect::Given(1), Effect::Resume(1)])
    );
    assert_eq!(
        state(&t, 1),
        SessionTakeOver {
            control: Control::Agent,
            call: Call::Running
        }
    );
    assert_eq!(t.taken(), None);

    // Handing back again does nothing, and doesn't resume the call again.
    assert_eq!(t.step(Event::Give), vec![]);
    assert_eq!(t.step(Event::Key(1, Key::Escape)), vec![]);
    assert_eq!(t.step(Event::Key(1, Key::Escape)), vec![]);
    assert_eq!(t.step(Event::Finish(1)), vec![]);
    assert_eq!(state(&t, 1), AGENT_IDLE);
    assert_eq!(t.step(Event::ToolCall(1)), vec![Effect::Admit(1)]);
}

#[test]
fn give_with_nothing_taken_over_does_nothing() {
    let mut t = connected(&[1, 2]);
    t.step(Event::ToolCall(2));
    let before = t.clone();
    assert_eq!(t.step(Event::Give), vec![]);
    assert_eq!(t.get(1), before.get(1));
    assert_eq!(t.get(2), before.get(2));
}

#[test]
fn escape_twice_hands_back_the_first_escape_goes_to_the_screen() {
    let mut t = connected(&[1]);
    t.step(Event::Click(1));
    t.step(Event::ToolCall(1));

    assert_eq!(t.step(Event::Key(1, Key::Escape)), vec![Effect::Forward(1)]);
    assert_eq!(state(&t, 1).control, Control::User);
    assert_eq!(
        sorted(t.step(Event::Key(1, Key::Escape))),
        sorted(vec![Effect::Given(1), Effect::Resume(1)]),
        "the second Escape must hand back, resume the waiting call, and go to no one"
    );
    assert_eq!(
        state(&t, 1),
        SessionTakeOver {
            control: Control::Agent,
            call: Call::Running
        }
    );
    assert_eq!(t.taken(), None);
}

#[test]
fn escape_twice_with_no_call_waiting_resumes_nothing() {
    let mut t = connected(&[1]);
    t.step(Event::Click(1));
    t.step(Event::Key(1, Key::Escape));
    assert_eq!(t.step(Event::Key(1, Key::Escape)), vec![Effect::Given(1)]);
    assert_eq!(state(&t, 1), AGENT_IDLE);
}

#[test]
fn a_key_or_click_between_the_escapes_starts_the_count_again() {
    for between in [Event::Key(1, Key::Other), Event::Click(1)] {
        let mut t = connected(&[1]);
        t.step(Event::Click(1));
        assert_eq!(t.step(Event::Key(1, Key::Escape)), vec![Effect::Forward(1)]);
        assert_eq!(t.step(between), vec![Effect::Forward(1)]);
        assert_eq!(
            t.step(Event::Key(1, Key::Escape)),
            vec![Effect::Forward(1)],
            "Escape, {between:?}, Escape handed back"
        );
        assert_eq!(state(&t, 1), USER_IDLE);
        assert_eq!(t.step(Event::Key(1, Key::Escape)), vec![Effect::Given(1)]);
    }
}

#[test]
fn pointer_motion_between_the_escapes_doesnt_break_the_pair() {
    let mut t = connected(&[1]);
    t.step(Event::Click(1));
    t.step(Event::Key(1, Key::Escape));
    assert_eq!(t.step(Event::Pointer(1)), vec![Effect::Forward(1)]);
    assert_eq!(t.step(Event::Key(1, Key::Escape)), vec![Effect::Given(1)]);
}

#[test]
fn an_escape_before_the_take_over_doesnt_count() {
    let mut t = connected(&[1]);
    // Pressed while the view was watch-only: dropped, and not remembered.
    t.step(Event::Key(1, Key::Escape));
    t.step(Event::Click(1));
    assert_eq!(t.step(Event::Key(1, Key::Escape)), vec![Effect::Forward(1)]);
    assert_eq!(state(&t, 1).control, Control::User);
}

#[test]
fn an_escape_from_an_earlier_take_over_doesnt_count() {
    let mut t = connected(&[1]);
    t.step(Event::Click(1));
    t.step(Event::Key(1, Key::Escape));
    t.step(Event::Give);
    t.step(Event::Click(1));
    assert_eq!(t.step(Event::Key(1, Key::Escape)), vec![Effect::Forward(1)]);
    assert_eq!(state(&t, 1).control, Control::User);
}

/// TLC's counterexample for TakeOverMidCall: a call is running when the user clicks in. Here the user gets the
/// screen only once the call is over, and nothing of theirs reaches it before.
#[test]
fn a_click_during_a_running_call_takes_over_once_the_call_ends() {
    let mut t = connected(&[1]);
    assert_eq!(t.step(Event::ToolCall(1)), vec![Effect::Admit(1)]);

    assert_eq!(
        t.step(Event::Click(1)),
        vec![],
        "took over with a call running"
    );
    assert_eq!(
        state(&t, 1),
        SessionTakeOver {
            control: Control::Taking,
            call: Call::Running
        }
    );
    assert_eq!(t.taken(), Some(1));
    for event in user_inputs(1) {
        assert_eq!(
            t.step(event),
            vec![],
            "{event:?} reached the screen mid-call"
        );
    }

    assert_eq!(t.step(Event::Finish(1)), vec![Effect::TakenOver(1)]);
    assert_eq!(state(&t, 1), USER_IDLE);
    assert_eq!(t.step(Event::Pointer(1)), vec![Effect::Forward(1)]);
}

/// AdmitWhileTaking: an agent that keeps calling must not keep the user out.
#[test]
fn an_agent_that_keeps_calling_cant_keep_the_user_out() {
    let mut t = connected(&[1]);
    t.step(Event::ToolCall(1));
    t.step(Event::Click(1));
    assert_eq!(
        t.step(Event::ToolCall(1)),
        vec![],
        "a call arrived with one running"
    );
    assert_eq!(t.step(Event::Finish(1)), vec![Effect::TakenOver(1)]);
    assert_eq!(t.step(Event::ToolCall(1)), vec![Effect::Wait(1)]);
    assert_eq!(state(&t, 1).control, Control::User);
}

#[test]
fn a_call_that_arrives_while_taking_waits() {
    let mut t = connected(&[1]);
    t.step(Event::ToolCall(1));
    t.step(Event::Click(1));
    t.step(Event::Finish(1));
    assert_eq!(t.step(Event::ToolCall(1)), vec![Effect::Wait(1)]);
}

#[test]
fn give_before_the_user_has_the_screen_cancels_the_take_over() {
    let mut t = connected(&[1]);
    t.step(Event::ToolCall(1));
    t.step(Event::Click(1));
    assert_eq!(t.step(Event::Give), vec![Effect::Given(1)]);
    assert_eq!(
        state(&t, 1),
        SessionTakeOver {
            control: Control::Agent,
            call: Call::Running
        }
    );
    assert_eq!(t.taken(), None);
    assert_eq!(t.step(Event::Finish(1)), vec![], "took over after all");
    assert_eq!(state(&t, 1), AGENT_IDLE);
}

#[test]
fn a_take_over_never_ends_by_anything_the_agent_does() {
    let mut t = connected(&[1]);
    t.step(Event::Click(1));
    t.step(Event::ToolCall(1));
    t.step(Event::ToolCall(1));
    t.step(Event::Finish(1));
    t.step(Event::Connect(1));
    assert_eq!(state(&t, 1).control, Control::User);
    assert_eq!(state(&t, 1).call, Call::Waiting);
}

#[test]
fn one_screen_at_a_time_clicking_into_another_hands_the_first_back() {
    let mut t = connected(&[1, 2]);
    t.step(Event::Click(1));
    t.step(Event::ToolCall(1));

    assert_eq!(
        sorted(t.step(Event::Click(2))),
        sorted(vec![
            Effect::Given(1),
            Effect::Resume(1),
            Effect::TakenOver(2)
        ])
    );
    assert_eq!(t.taken(), Some(2));
    assert_eq!(state(&t, 2), USER_IDLE);
    assert_eq!(
        state(&t, 1),
        SessionTakeOver {
            control: Control::Agent,
            call: Call::Running
        }
    );
    // The first view is watch-only again.
    assert_eq!(t.step(Event::Key(1, Key::Other)), vec![]);
    assert_eq!(t.step(Event::Key(2, Key::Other)), vec![Effect::Forward(2)]);
}

#[test]
fn take_of_another_agent_hands_the_first_back() {
    let mut t = connected(&[1, 2]);
    t.step(Event::Take(1));
    assert_eq!(
        sorted(t.step(Event::Take(2))),
        sorted(vec![Effect::Given(1), Effect::TakenOver(2)])
    );
    assert_eq!(t.taken(), Some(2));
    assert_eq!(state(&t, 1), AGENT_IDLE);
}

#[test]
fn input_in_another_agents_view_doesnt_reach_the_screen_taken_over() {
    let mut t = connected(&[1, 2]);
    t.step(Event::Click(1));
    for event in [
        Event::Pointer(2),
        Event::Key(2, Key::Other),
        Event::Key(2, Key::Escape),
        Event::Key(2, Key::Escape),
    ] {
        assert_eq!(t.step(event), vec![], "{event:?}");
    }
    assert_eq!(t.taken(), Some(1));
    assert_eq!(state(&t, 1), USER_IDLE);
}

#[test]
fn taking_over_one_agent_leaves_the_others_alone() {
    let mut t = connected(&[1, 2, 3]);
    t.step(Event::ToolCall(3));
    let before = t.clone();
    t.step(Event::Click(1));
    t.step(Event::ToolCall(1));
    assert_eq!(t.get(2), before.get(2));
    assert_eq!(t.get(3), before.get(3));
    assert_eq!(t.step(Event::ToolCall(2)), vec![Effect::Admit(2)]);
    assert_eq!(t.step(Event::Finish(3)), vec![]);
    assert_eq!(t.step(Event::ToolCall(3)), vec![Effect::Admit(3)]);
    t.step(Event::Give);
    assert_eq!(state(&t, 2).call, Call::Running);
    assert_eq!(state(&t, 3).call, Call::Running);
}

#[test]
fn a_session_that_ends_during_a_take_over_ends_it() {
    let mut t = connected(&[1, 2]);
    t.step(Event::Click(1));
    t.step(Event::ToolCall(1));

    assert_eq!(
        t.step(Event::Disconnect(1)),
        vec![Effect::Given(1)],
        "the waiting call has no one to answer, and isn't resumed"
    );
    assert_eq!(t.get(1), None);
    assert_eq!(t.taken(), None);
    assert_eq!(t.step(Event::Give), vec![]);
    // Late events of the gone session do nothing.
    for event in [
        Event::Finish(1),
        Event::Click(1),
        Event::Key(1, Key::Escape),
        Event::Take(1),
    ] {
        assert_eq!(t.step(event), vec![], "{event:?}");
    }
    // The user can take over another agent.
    assert_eq!(t.step(Event::Click(2)), vec![Effect::TakenOver(2)]);
}

#[test]
fn a_session_that_ends_while_taking_ends_it() {
    let mut t = connected(&[1]);
    t.step(Event::ToolCall(1));
    t.step(Event::Click(1));
    assert_eq!(t.step(Event::Disconnect(1)), vec![Effect::Given(1)]);
    assert_eq!(t.taken(), None);
    t.step(Event::Connect(2));
    assert_eq!(state(&t, 2), AGENT_IDLE);
}

#[test]
fn a_session_that_ends_outside_a_take_over_gives_nothing_back() {
    let mut t = connected(&[1, 2]);
    t.step(Event::Click(2));
    t.step(Event::ToolCall(1));
    assert_eq!(t.step(Event::Disconnect(1)), vec![]);
    assert_eq!(t.taken(), Some(2));
}

#[test]
fn events_for_an_unknown_agent_change_nothing() {
    let mut t = connected(&[1]);
    t.step(Event::Click(1));
    let before = t.clone();
    for event in [
        Event::Disconnect(9),
        Event::ToolCall(9),
        Event::Finish(9),
        Event::Click(9),
        Event::Pointer(9),
        Event::Key(9, Key::Escape),
        Event::Key(9, Key::Other),
        Event::Take(9),
    ] {
        assert_eq!(t.step(event), vec![], "{event:?}");
    }
    assert_eq!(t.get(9), None);
    assert_eq!(t.get(1), before.get(1));
    assert_eq!(t.taken(), Some(1));
}

#[test]
fn connecting_a_known_agent_again_changes_nothing() {
    let mut t = connected(&[1]);
    t.step(Event::Click(1));
    t.step(Event::ToolCall(1));
    let before = t.get(1);
    assert_eq!(t.step(Event::Connect(1)), vec![]);
    assert_eq!(t.get(1), before);
}

#[test]
fn finish_without_a_running_call_changes_nothing() {
    let mut t = connected(&[1]);
    assert_eq!(t.step(Event::Finish(1)), vec![]);
    assert_eq!(state(&t, 1), AGENT_IDLE);
    t.step(Event::Click(1));
    t.step(Event::ToolCall(1));
    assert_eq!(t.step(Event::Finish(1)), vec![], "a waiting call finished");
    assert_eq!(state(&t, 1).call, Call::Waiting);
}
