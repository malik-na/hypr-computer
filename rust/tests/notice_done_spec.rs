//! #29: when an agent's session ends, its notice is replaced by a
//! short-lived "done" notice, so that none outlives its session, even on a
//! desktop that keeps a closed notification on screen. Written from the spec
//! (specs/Notice.tla and the issue) alone; spec29/ says what interface it
//! assumes.
#![forbid(unsafe_code)]

mod spec29;
mod support;

use std::thread;
use std::time::Duration;

use spec29::{Agent, Notify, Service, World};

/// Fails the test unless `done` is `agent`'s "done" notice in place of
/// notification `id`.
#[track_caller]
fn assert_done(done: &Notify, id: u32, agent: &Agent) {
    assert_eq!(done.replaces_id, id, "{done:?}");
    assert!(
        done.is_short_lived(),
        "\"done\" must go away by itself within 10 s, with no buttons: {done:?}"
    );
    let text = done.text();
    assert!(text.contains(&agent.label()), "{text:?}");
    assert!(text.to_lowercase().contains("done"), "{text:?}");
}

/// Fails the test unless nothing lasting of `agent`'s is on screen: what is
/// left of its notice goes away by itself.
#[track_caller]
fn assert_nothing_lasting_of(world: &World, agent: &Agent) {
    let label = agent.label();
    for (id, shown) in world.notifications().on_screen() {
        assert!(
            !shown.text().contains(&label) || shown.is_short_lived(),
            "{label}'s notice {id} outlives its session: {shown:?}"
        );
    }
}

#[test]
fn when_the_agent_exits_its_notice_is_replaced_by_a_short_lived_done_notice() {
    let world = World::start(Service::Up);
    // As Omarchy's shell does: closing the notice wouldn't take it away.
    world.notifications().keep_closed();
    let mut alpha = world.agent("alpha");
    let (id, _) = world.notice_of(&alpha);
    alpha.assert_works();

    alpha.exit();

    let done = world.wait_for_replacement_of(id);
    assert_done(&done, id, &alpha);
    assert_nothing_lasting_of(&world, &alpha);
}

#[test]
fn when_the_agent_is_killed_its_notice_is_replaced_too() {
    let world = World::start(Service::Up);
    world.notifications().keep_closed();
    let mut alpha = world.agent("alpha");
    let (id, _) = world.notice_of(&alpha);

    alpha.kill();

    let done = world.wait_for_replacement_of(id);
    assert_done(&done, id, &alpha);
    assert_nothing_lasting_of(&world, &alpha);
}

#[test]
fn one_agent_ending_leaves_the_others_notice_on_screen() {
    let world = World::start(Service::Up);
    world.notifications().keep_closed();
    let mut alpha = world.agent("alpha");
    let mut beta = world.agent("beta");
    let (alpha_id, _) = world.notice_of(&alpha);
    let (beta_id, beta_notice) = world.notice_of(&beta);

    alpha.exit();
    world.wait_for_replacement_of(alpha_id);
    thread::sleep(Duration::from_secs(1));

    assert!(
        world.replacements_of(beta_id).is_empty(),
        "beta's notice was replaced when alpha ended"
    );
    assert!(!world.notifications().closes().contains(&beta_id));
    let shown = world.notifications().on_screen();
    assert_eq!(
        shown.get(&beta_id).map(Notify::text),
        Some(beta_notice.text()),
        "beta's notice must stay while beta works"
    );
    beta.assert_works();
}

#[test]
fn each_agent_gets_one_done_notice() {
    let world = World::start(Service::Up);
    let mut alpha = world.agent("alpha");
    let (id, _) = world.notice_of(&alpha);

    alpha.exit();
    world.wait_for_replacement_of(id);
    thread::sleep(Duration::from_secs(1));

    let notifies = world.notifications().notifies();
    assert_eq!(
        notifies.len(),
        2,
        "one notice and one \"done\": {notifies:#?}"
    );
}

/// When the daemon is stopped (systemd's SIGTERM, at logout or on update),
/// every session ends, and so does every notice.
#[test]
fn stopping_the_daemon_replaces_every_notice() {
    let mut world = World::start(Service::Up);
    world.notifications().keep_closed();
    let alpha = world.agent("alpha");
    let beta = world.agent("beta");
    let (alpha_id, _) = world.notice_of(&alpha);
    let (beta_id, _) = world.notice_of(&beta);

    world.daemon.terminate();

    for (id, agent) in [(alpha_id, &alpha), (beta_id, &beta)] {
        let done = world.wait_for_replacement_of(id);
        assert_done(&done, id, agent);
        assert_nothing_lasting_of(&world, agent);
    }
}

/// TLC's counterexample for ForgetLateAnswer: the agent leaves before the
/// service has answered, and the service then shows the notice, which never
/// expires. The daemon must still be waiting, and replace it at once.
#[test]
fn a_notice_shown_after_its_session_ended_is_replaced_at_once() {
    let world = World::start(Service::Hung);
    world.notifications().keep_closed();
    let mut alpha = world.agent("alpha");
    world.asked_for_notice_of(&alpha);

    alpha.exit();
    thread::sleep(Duration::from_millis(500));
    world.notifications().release();

    let (id, _) = world.notice_of(&alpha);
    let done = world.wait_for_replacement_of(id);
    assert_done(&done, id, &alpha);
    assert_nothing_lasting_of(&world, &alpha);
}
