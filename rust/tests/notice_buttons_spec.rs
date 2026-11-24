//! #29: the notice's buttons. Stop stops that agent, and no other, with
//! #28's stop: `hypr-computer resume` lets it go on. OK, or closing the
//! notice, lets the agent go on. Written from the spec (specs/Notice.tla and
//! the issue) alone; spec29/ says what interface it assumes.
#![forbid(unsafe_code)]

mod spec29;
mod support;

use std::thread;
use std::time::Duration;

use spec29::{Service, World};

#[test]
fn stop_on_an_agents_notice_stops_that_agent_and_no_other() {
    let world = World::start(Service::Up);
    let mut alpha = world.agent("alpha");
    let mut beta = world.agent("beta");
    let (alpha_id, _) = world.notice_of(&alpha);
    world.notice_of(&beta);
    alpha.assert_works();

    world.notifications().press(alpha_id, "stop");

    alpha.assert_becomes_stopped();
    beta.assert_works();
    beta.assert_works();
    assert!(alpha.is_stopped(), "alpha must stay stopped");
}

/// Stop on a notice isn't `hypr-computer stop` of everyone, which also stops
/// every agent that connects until `hypr-computer resume`.
#[test]
fn stop_on_a_notice_stops_no_agent_that_connects_after() {
    let world = World::start(Service::Up);
    let mut alpha = world.agent("alpha");
    let (alpha_id, _) = world.notice_of(&alpha);

    world.notifications().press(alpha_id, "stop");
    alpha.assert_becomes_stopped();

    let mut beta = world.agent("beta");
    world.notice_of(&beta);
    beta.assert_works();
}

/// The notice's Stop is `hypr-computer stop <agent>`, not a stop of its own:
/// `hypr-computer resume` undoes it.
#[test]
fn an_agent_stopped_from_its_notice_goes_on_after_hypr_computer_resume() {
    let world = World::start(Service::Up);
    let mut alpha = world.agent("alpha");
    let (id, _) = world.notice_of(&alpha);

    world.notifications().press(id, "stop");
    alpha.assert_becomes_stopped();

    world.user_runs(&["resume", "alpha"]);
    alpha.assert_works();
}

#[test]
fn an_agent_stopped_from_its_notice_goes_on_after_hypr_computer_resume_of_all() {
    let world = World::start(Service::Up);
    let mut alpha = world.agent("alpha");
    let (id, _) = world.notice_of(&alpha);

    world.notifications().press(id, "stop");
    alpha.assert_becomes_stopped();

    world.user_runs(&["resume"]);
    alpha.assert_works();
}

/// Two agents of the same kind share a name: Stop on the second one's notice
/// stops the second one only, as `hypr-computer stop 2` would.
#[test]
fn stop_on_the_notice_of_one_of_two_agents_of_the_same_name_stops_only_that_one() {
    let world = World::start(Service::Up);
    let mut first = world.agent("claude-code");
    let mut second = world.agent("claude-code");
    world.notice_of(&first);
    let (second_id, _) = world.notice_of(&second);

    world.notifications().press(second_id, "stop");

    second.assert_becomes_stopped();
    first.assert_works();
}

#[test]
fn ok_lets_the_agent_go_on() {
    let world = World::start(Service::Up);
    let mut alpha = world.agent("alpha");
    let (id, _) = world.notice_of(&alpha);

    world.notifications().press(id, "ok");
    thread::sleep(Duration::from_millis(500));

    alpha.assert_works();
    alpha.assert_works();
}

#[test]
fn closing_the_notice_lets_the_agent_go_on() {
    let world = World::start(Service::Up);
    let mut alpha = world.agent("alpha");
    let (id, _) = world.notice_of(&alpha);

    world.notifications().dismiss(id);
    thread::sleep(Duration::from_millis(500));

    alpha.assert_works();
}

/// Once the user has answered the notice it is gone: the session's end has
/// nothing to replace, and no "done" pops up.
#[test]
fn after_the_user_answers_the_notice_the_agents_end_shows_nothing_new() {
    for answer in ["ok", "stop", "closed"] {
        let world = World::start(Service::Up);
        let mut alpha = world.agent("alpha");
        let (id, _) = world.notice_of(&alpha);
        if answer == "closed" {
            world.notifications().dismiss(id);
        } else {
            world.notifications().press(id, answer);
        }
        if answer == "stop" {
            alpha.assert_becomes_stopped();
        } else {
            thread::sleep(Duration::from_millis(500));
            alpha.assert_works();
        }

        alpha.exit();
        // A second agent's notice tells when the daemon has moved on.
        let beta = world.agent("beta");
        world.notice_of(&beta);
        thread::sleep(Duration::from_secs(1));

        let notifies = world.notifications().notifies();
        assert!(
            notifies
                .iter()
                .all(|n| !n.text().contains(&alpha.label()) || n.replaces_id == 0),
            "after {answer}, alpha's end showed a notice: {notifies:#?}"
        );
        assert_eq!(world.notices_of(&alpha).len(), 1, "after {answer}");
    }
}

/// Other apps' notifications have buttons too, and the service tells every
/// listener about them.
#[test]
fn buttons_of_other_notifications_stop_nothing() {
    let world = World::start(Service::Up);
    let mut alpha = world.agent("alpha");
    let (id, _) = world.notice_of(&alpha);

    world.notifications().action_invoked(id + 100, "stop");
    world.notifications().action_invoked(id + 101, "ok");
    world.notifications().closed(id + 100, 2);
    // A button of alpha's notice that isn't Stop.
    world.notifications().action_invoked(id, "default");
    thread::sleep(Duration::from_millis(500));

    alpha.assert_works();

    // Its own Stop still works afterwards.
    world.notifications().press(id, "stop");
    alpha.assert_becomes_stopped();
}

/// After a stop from the notice, `hypr-computer stop` of everyone and its
/// `resume` work as they always do.
#[test]
fn stop_from_the_notice_and_hypr_computer_stop_agree() {
    let world = World::start(Service::Up);
    let mut alpha = world.agent("alpha");
    let mut beta = world.agent("beta");
    let (alpha_id, _) = world.notice_of(&alpha);
    world.notice_of(&beta);

    world.notifications().press(alpha_id, "stop");
    alpha.assert_becomes_stopped();
    world.user_runs(&["stop"]);
    beta.assert_becomes_stopped();

    world.user_runs(&["resume", "beta"]);
    beta.assert_works();
    assert!(alpha.is_stopped(), "resuming beta resumed alpha");
}
