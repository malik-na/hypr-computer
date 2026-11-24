//! The notice that tells the user an agent is at work, tested at its seam: `domain::notice`, against
//! specs/Notice.tla.
//!
//! # The API these tests fix
//!
//! `rust/crates/domain/src/notice.rs`, declared `pub mod notice;` in `lib.rs`. It holds what the daemon believes
//! about each session's notice: the spec's `notice`. The notification service, the host desktop and the user are
//! the world; what they do comes in as events, and what the daemon must do to them goes out as effects.
//!
//! ```ignore
//! /// What the daemon believes about a session's notice. The spec's `notice`; its "None" is `get` giving `None`.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Notice<N> {
//!     /// It asked the service to show the notice, and waits for the answer for a limited time.
//!     Asking,
//!     /// The service showed it, with this id.
//!     Shown(N),
//!     /// No answer in time: the host desktop showed its notice. Still waiting for the service's answer.
//!     Late,
//!     /// The service failed: the host desktop showed its notice.
//!     Fallback,
//!     /// The user pressed OK or Stop on it, or closed it: it is gone.
//!     Answered,
//!     /// The session ended while the daemon still waited for the answer: when it comes, it shows "done".
//!     Ending,
//! }
//!
//! /// What the user did with a notice.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Answer {
//!     /// Pressed OK: `ActionInvoked(id, "ok")`.
//!     Ok,
//!     /// Pressed Stop: `ActionInvoked(id, "stop")`.
//!     Stop,
//!     /// Closed it: `NotificationClosed(id, _)`.
//!     Closed,
//! }
//!
//! /// Something that happened to session `K`'s notice (the daemon uses `SessionId`), whose id with the
//! /// notification service is an `N` (the daemon uses `u32`).
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Event<K, N> {
//!     /// The session is open and its agent has said its name.
//!     Connect(K),
//!     /// The session is gone.
//!     Disconnect(K),
//!     /// The service answered the session's Notify with this id: the notice is on screen.
//!     Shown(K, N),
//!     /// The session's Notify failed: no service, no session bus, or an error.
//!     Failed(K),
//!     /// The time the daemon waits for the service's answer is over.
//!     Timeout(K),
//!     /// The user answered the notification with this id.
//!     Answer(N, Answer),
//! }
//!
//! /// What the daemon must do because of an event.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum Effect<K, N> {
//!     /// Ask the service to show the session's notice (Notify: OK and Stop, never expiring), and start the timer.
//!     /// Step `Shown` or `Failed` when it answers, and `Timeout` when the timer fires.
//!     Show(K),
//!     /// Have the host desktop show its own notice for the session; it goes away by itself.
//!     Host(K),
//!     /// Replace notification `N` with the session's short-lived "done" notice (Notify replacing `N`, no
//!     /// actions, expiring).
//!     Done(K, N),
//!     /// Stop the session, as `hypr-computer stop <agent>` does: step `stop::Event::Stop(K)`.
//!     Stop(K),
//! }
//!
//! /// The notice of every session the daemon still has something to do for.
//! #[derive(Clone, Debug)]          // PartialEq, Eq and Default are welcome, not required
//! pub struct Notices<K, N> { /* private */ }
//!
//! impl<K: Copy + Ord, N: Copy + Ord> Notices<K, N> {
//!     /// No session.
//!     pub fn new() -> Self;
//!     /// Applies one event, and says what to do about it. Pure: no I/O, no clock.
//!     pub fn step(&mut self, event: Event<K, N>) -> Vec<Effect<K, N>>;
//!     /// The session's notice; `None` before its `Connect`, and once nothing is left to do for it.
//!     pub fn get(&self, id: K) -> Option<Notice<N>>;
//! }
//! ```
//!
//! `step`, for a session `k` whose notice is `s` before the event. Every case not listed changes nothing and
//! returns no effect: an event for an unknown `k`, a `Connect` of a known one, `Shown`, `Failed` or `Timeout`
//! when nothing waits for them, and an `Answer` for an id that no session's notice is `Shown` with.
//!
//! | Event | When | Then | Effects |
//! | --- | --- | --- | --- |
//! | `Connect(k)` | `k` unknown | `Asking` | `Show(k)` |
//! | `Disconnect(k)` | `Asking` or `Late` | `Ending` | none |
//! | `Disconnect(k)` | `Shown(n)` | forgotten | `Done(k, n)` |
//! | `Disconnect(k)` | `Fallback` or `Answered` | forgotten | none |
//! | `Shown(k, n)` | `Asking` or `Late` | `Shown(n)` | none: a late answer takes the place of the host's notice |
//! | `Shown(k, n)` | `Ending` | forgotten | `Done(k, n)` |
//! | `Failed(k)` | `Asking` | `Fallback` | `Host(k)` |
//! | `Failed(k)` | `Late` | `Fallback` | none: the host's notice is already shown |
//! | `Failed(k)` | `Ending` | forgotten | none |
//! | `Timeout(k)` | `Asking` | `Late` | `Host(k)` |
//! | `Answer(n, Ok)`, `Answer(n, Closed)` | `k` is `Shown(n)` | `Answered` | none |
//! | `Answer(n, Stop)` | `k` is `Shown(n)` | `Answered` | `Stop(k)` |
//!
//! So a session's notice is asked for once; the host shows its notice once at most; "done" replaces a notice
//! only once its session has ended, and only one the user hasn't answered; and Stop stops only the session whose
//! notice it was on, while it is open.
//!
//! What the caller must do, because `step` can't check it:
//! - **One `Notices` per daemon, owned by one task, stepped one event at a time.** `Connect` a fresh id once the
//!   session's agent has said its name, and `Disconnect` it when the session is gone. Ids aren't reused.
//! - **On `Show(k)`, never drop the Notify call.** The timer only brings the host's notice: the call goes on, and
//!   its answer is stepped whenever it comes, even after the session has ended, so that a late notice is
//!   replaced by "done".
//! - **Step the service's messages in the order they came.** A Notify answer before an `ActionInvoked` or
//!   `NotificationClosed` that came after it, so that the user's Stop on a notice that has just appeared isn't
//!   lost.
//! - **On `Stop(k)`,** step `stop::Event::Stop(k)` on the daemon's `Stops`: the notice's Stop is
//!   `hypr-computer stop <agent>`, not a second mechanism.
//! - **When the daemon stops** (SIGTERM), it ends every session, so every notice on screen is replaced by "done".
#![forbid(unsafe_code)]

use domain::notice::{Answer, Effect, Event, Notice, Notices};

type Id = u32;
type Note = u32;

fn connected(ids: &[Id]) -> Notices<Id, Note> {
    let mut notices = Notices::new();
    for &id in ids {
        assert_eq!(notices.step(Event::Connect(id)), vec![Effect::Show(id)]);
    }
    notices
}

/// Session `id`'s notice, shown by the service with id `note`.
fn shown(notices: &mut Notices<Id, Note>, id: Id, note: Note) {
    assert_eq!(notices.step(Event::Shown(id, note)), vec![]);
    assert_eq!(notices.get(id), Some(Notice::Shown(note)));
}

#[test]
fn an_agent_that_connects_gets_one_notice_asked_for() {
    let mut notices = connected(&[1]);
    assert_eq!(notices.get(1), Some(Notice::Asking));

    // Saying it again asks for nothing more.
    assert_eq!(notices.step(Event::Connect(1)), vec![]);
    assert_eq!(notices.get(1), Some(Notice::Asking));
}

#[test]
fn nothing_is_known_about_a_session_before_it_connects() {
    let mut notices: Notices<Id, Note> = Notices::new();
    assert_eq!(notices.get(1), None);
    for event in [
        Event::Disconnect(1),
        Event::Shown(1, 7),
        Event::Failed(1),
        Event::Timeout(1),
        Event::Answer(7, Answer::Stop),
    ] {
        assert_eq!(
            notices.step(event),
            vec![],
            "{event:?} for an unknown session"
        );
        assert_eq!(notices.get(1), None);
    }
}

#[test]
fn the_service_shows_the_notice_and_the_session_ends_with_done() {
    let mut notices = connected(&[1]);
    shown(&mut notices, 1, 7);

    assert_eq!(notices.step(Event::Disconnect(1)), vec![Effect::Done(1, 7)]);
    assert_eq!(notices.get(1), None);
}

#[test]
fn ok_takes_the_notice_away_and_stops_nothing() {
    let mut notices = connected(&[1]);
    shown(&mut notices, 1, 7);

    assert_eq!(notices.step(Event::Answer(7, Answer::Ok)), vec![]);
    assert_eq!(notices.get(1), Some(Notice::Answered));

    // Nothing left to replace when the session ends.
    assert_eq!(notices.step(Event::Disconnect(1)), vec![]);
    assert_eq!(notices.get(1), None);
}

#[test]
fn closing_the_notice_is_like_ok() {
    let mut notices = connected(&[1]);
    shown(&mut notices, 1, 7);

    assert_eq!(notices.step(Event::Answer(7, Answer::Closed)), vec![]);
    assert_eq!(notices.get(1), Some(Notice::Answered));
    assert_eq!(notices.step(Event::Disconnect(1)), vec![]);
}

#[test]
fn stop_stops_the_notices_own_session() {
    let mut notices = connected(&[1]);
    shown(&mut notices, 1, 7);

    assert_eq!(
        notices.step(Event::Answer(7, Answer::Stop)),
        vec![Effect::Stop(1)]
    );
    assert_eq!(notices.get(1), Some(Notice::Answered));

    // The service closes the notification after the action: that changes nothing more.
    assert_eq!(notices.step(Event::Answer(7, Answer::Closed)), vec![]);
    // And a second Stop on a notice that is gone stops nothing.
    assert_eq!(notices.step(Event::Answer(7, Answer::Stop)), vec![]);
    assert_eq!(notices.step(Event::Disconnect(1)), vec![]);
}

/// TLC's counterexample for StopStopsAll: a notice is shown, the user presses Stop, a second agent connects, and
/// the daemon stops both. Here only the first is stopped.
#[test]
fn stop_on_one_agents_notice_stops_no_other() {
    let mut notices = connected(&[1]);
    shown(&mut notices, 1, 7);
    assert_eq!(notices.step(Event::Connect(2)), vec![Effect::Show(2)]);
    shown(&mut notices, 2, 8);

    assert_eq!(
        notices.step(Event::Answer(7, Answer::Stop)),
        vec![Effect::Stop(1)]
    );
    assert_eq!(notices.get(2), Some(Notice::Shown(8)));

    assert_eq!(
        notices.step(Event::Answer(8, Answer::Stop)),
        vec![Effect::Stop(2)]
    );
}

#[test]
fn an_answer_on_another_apps_notification_changes_nothing() {
    let mut notices = connected(&[1]);
    shown(&mut notices, 1, 7);

    for answer in [Answer::Ok, Answer::Stop, Answer::Closed] {
        assert_eq!(notices.step(Event::Answer(99, answer)), vec![]);
    }
    assert_eq!(notices.get(1), Some(Notice::Shown(7)));
}

#[test]
fn stop_on_a_notice_whose_session_has_ended_stops_nothing() {
    let mut notices = connected(&[1]);
    shown(&mut notices, 1, 7);
    assert_eq!(notices.step(Event::Disconnect(1)), vec![Effect::Done(1, 7)]);

    // The user pressed Stop just before "done" replaced it.
    assert_eq!(notices.step(Event::Answer(7, Answer::Stop)), vec![]);
    assert_eq!(notices.get(1), None);
}

#[test]
fn without_a_service_the_host_desktop_shows_the_notice() {
    let mut notices = connected(&[1]);
    assert_eq!(notices.step(Event::Failed(1)), vec![Effect::Host(1)]);
    assert_eq!(notices.get(1), Some(Notice::Fallback));

    // Its own time takes it away: nothing to replace when the session ends.
    assert_eq!(notices.step(Event::Disconnect(1)), vec![]);
    assert_eq!(notices.get(1), None);
}

/// TLC's counterexample for WaitForever: a hung service, and no notice ever. Here the timer brings the host's.
#[test]
fn a_service_that_hangs_gets_the_host_desktop_to_show_the_notice() {
    let mut notices = connected(&[1]);
    assert_eq!(notices.step(Event::Timeout(1)), vec![Effect::Host(1)]);
    assert_eq!(notices.get(1), Some(Notice::Late));

    // A second timer, or the call failing after all, shows nothing more.
    assert_eq!(notices.step(Event::Timeout(1)), vec![]);
    assert_eq!(notices.step(Event::Failed(1)), vec![]);
    assert_eq!(notices.get(1), Some(Notice::Fallback));
}

#[test]
fn a_late_answer_takes_the_place_of_the_hosts_notice() {
    let mut notices = connected(&[1]);
    assert_eq!(notices.step(Event::Timeout(1)), vec![Effect::Host(1)]);

    assert_eq!(notices.step(Event::Shown(1, 7)), vec![]);
    assert_eq!(notices.get(1), Some(Notice::Shown(7)));

    // Its buttons work, and the session's end replaces it.
    assert_eq!(
        notices.clone().step(Event::Answer(7, Answer::Stop)),
        vec![Effect::Stop(1)]
    );
    assert_eq!(notices.step(Event::Disconnect(1)), vec![Effect::Done(1, 7)]);
}

#[test]
fn a_timer_after_the_answer_changes_nothing() {
    let mut notices = connected(&[1]);
    shown(&mut notices, 1, 7);
    assert_eq!(notices.step(Event::Timeout(1)), vec![]);
    assert_eq!(notices.get(1), Some(Notice::Shown(7)));

    let mut failed = connected(&[2]);
    assert_eq!(failed.step(Event::Failed(2)), vec![Effect::Host(2)]);
    assert_eq!(failed.step(Event::Timeout(2)), vec![]);
    assert_eq!(failed.step(Event::Shown(2, 9)), vec![]);
    assert_eq!(failed.get(2), Some(Notice::Fallback));
}

/// TLC's counterexample for ForgetLateAnswer: the session ends before the service has answered; the service then
/// shows the notice. Here the daemon still waits, and replaces it with "done" as soon as its id comes.
#[test]
fn a_notice_whose_id_comes_after_its_session_ended_is_replaced_at_once() {
    let mut notices = connected(&[1]);
    assert_eq!(notices.step(Event::Disconnect(1)), vec![]);
    assert_eq!(notices.get(1), Some(Notice::Ending));

    assert_eq!(notices.step(Event::Shown(1, 7)), vec![Effect::Done(1, 7)]);
    assert_eq!(notices.get(1), None);
}

#[test]
fn a_notice_that_fell_back_and_whose_session_ended_is_still_replaced_when_its_id_comes() {
    let mut notices = connected(&[1]);
    assert_eq!(notices.step(Event::Timeout(1)), vec![Effect::Host(1)]);
    assert_eq!(notices.step(Event::Disconnect(1)), vec![]);
    assert_eq!(notices.get(1), Some(Notice::Ending));

    assert_eq!(notices.step(Event::Shown(1, 7)), vec![Effect::Done(1, 7)]);
    assert_eq!(notices.get(1), None);
}

#[test]
fn a_session_that_ended_before_its_notice_failed_is_forgotten_without_a_host_notice() {
    let mut notices = connected(&[1]);
    assert_eq!(notices.step(Event::Disconnect(1)), vec![]);
    assert_eq!(notices.step(Event::Failed(1)), vec![]);
    assert_eq!(notices.get(1), None);

    // Its timer, firing after, shows nothing either.
    assert_eq!(notices.step(Event::Timeout(1)), vec![]);
}

#[test]
fn an_ending_session_shows_no_host_notice_when_its_timer_fires() {
    let mut notices = connected(&[1]);
    assert_eq!(notices.step(Event::Disconnect(1)), vec![]);
    assert_eq!(notices.step(Event::Timeout(1)), vec![]);
    assert_eq!(notices.get(1), Some(Notice::Ending));
}

/// TLC's counterexample for CloseOnEnd: the session ends with its notice on screen, and the daemon closes it,
/// which Omarchy's shell ignores. Here it is replaced by "done", which expires whatever the service.
#[test]
fn a_session_that_ends_with_its_notice_on_screen_replaces_it_and_closes_nothing() {
    let mut notices = connected(&[1, 2]);
    shown(&mut notices, 1, 7);
    shown(&mut notices, 2, 8);

    assert_eq!(notices.step(Event::Disconnect(2)), vec![Effect::Done(2, 8)]);
    assert_eq!(notices.get(1), Some(Notice::Shown(7)));
    assert_eq!(notices.step(Event::Disconnect(1)), vec![Effect::Done(1, 7)]);
}

#[test]
fn a_second_disconnect_changes_nothing() {
    let mut notices = connected(&[1]);
    shown(&mut notices, 1, 7);
    assert_eq!(notices.step(Event::Disconnect(1)), vec![Effect::Done(1, 7)]);
    assert_eq!(notices.step(Event::Disconnect(1)), vec![]);

    let mut ending = connected(&[2]);
    assert_eq!(ending.step(Event::Disconnect(2)), vec![]);
    assert_eq!(ending.step(Event::Disconnect(2)), vec![]);
    assert_eq!(ending.get(2), Some(Notice::Ending));
}

#[test]
fn sessions_are_independent() {
    let mut notices = connected(&[1, 2, 3]);
    shown(&mut notices, 1, 7);
    assert_eq!(notices.step(Event::Timeout(2)), vec![Effect::Host(2)]);
    assert_eq!(notices.step(Event::Failed(3)), vec![Effect::Host(3)]);

    assert_eq!(notices.get(1), Some(Notice::Shown(7)));
    assert_eq!(notices.get(2), Some(Notice::Late));
    assert_eq!(notices.get(3), Some(Notice::Fallback));

    assert_eq!(notices.step(Event::Disconnect(2)), vec![]);
    assert_eq!(notices.get(1), Some(Notice::Shown(7)));
    assert_eq!(notices.get(3), Some(Notice::Fallback));
}
