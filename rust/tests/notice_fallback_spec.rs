//! #29: without a notification service, or with one that fails or hangs
//! instead of answering, the host desktop shows its own notice, and the
//! daemon works as before. Written from the spec (specs/Notice.tla and the
//! issue) alone; spec29/ says what interface it assumes.
#![forbid(unsafe_code)]

mod spec29;
mod support;

use std::thread;
use std::time::{Duration, Instant};

use spec29::{Agent, HostNotice, Service, TOLD, World, says_where_to_watch};

/// How long a call may take while the notification service hangs: the
/// screen answers in milliseconds, so this only fails a daemon that waits for
/// the service.
const PROMPT: Duration = Duration::from_secs(2);

/// Fails the test unless `notice` is the host desktop's notice for `agent`.
#[track_caller]
fn assert_host_notice(notice: &HostNotice, agent: &Agent) {
    assert!(
        notice.time_ms > 0,
        "the host's notice must go away by itself: {notice:?}"
    );
    let text = &notice.message;
    assert!(text.contains(&agent.label()), "{text:?}");
    assert!(
        says_where_to_watch(text),
        "the host's notice must say where to watch the agent: {text:?}"
    );
    assert!(
        text.contains("hypr-computer stop"),
        "with no Stop button, the host's notice must say how to stop the agent: {text:?}"
    );
}

/// The host desktop's one notice for `agent`, once it has come.
#[track_caller]
fn told_by_the_host(world: &World, agent: &Agent) -> HostNotice {
    let notices = world.wait_for_host_notice_of(agent);
    assert_eq!(notices.len(), 1, "one notice per agent: {notices:?}");
    assert_host_notice(&notices[0], agent);
    notices[0].clone()
}

#[test]
fn without_a_session_bus_the_host_desktop_shows_the_notice() {
    let world = World::on_desktop(Service::NoBus);
    let mut alpha = world.agent("alpha");

    told_by_the_host(&world, &alpha);
    alpha.assert_works();
}

#[test]
fn without_a_notification_service_the_host_desktop_shows_the_notice() {
    let world = World::on_desktop(Service::Absent);
    let mut alpha = world.agent("alpha");

    told_by_the_host(&world, &alpha);
    alpha.assert_works();
}

#[test]
fn when_the_service_fails_the_host_desktop_shows_the_notice() {
    let world = World::on_desktop(Service::Failing);
    let mut alpha = world.agent("alpha");

    world.asked_for_notice_of(&alpha);
    told_by_the_host(&world, &alpha);
    alpha.assert_works();
}

/// TLC's counterexample for WaitForever: the service takes the call and
/// never answers. The Python server's notify-send did this with no service to
/// reach, and no notice ever appeared.
#[test]
fn when_the_service_hangs_the_host_desktop_shows_the_notice_and_nothing_waits_for_it() {
    let world = World::on_desktop(Service::Hung);
    let mut alpha = world.agent("alpha");
    world.asked_for_notice_of(&alpha);
    alpha.assert_works();

    // Another agent's notice hangs too, and neither agent waits for it.
    let mut beta = world.agent("beta");
    for agent in [&mut alpha, &mut beta] {
        let start = Instant::now();
        agent.assert_works();
        let took = start.elapsed();
        assert!(
            took < PROMPT || agent.session == 2,
            "a call took {took:?} while the service hangs"
        );
    }
    let start = Instant::now();
    alpha.assert_works();
    assert!(start.elapsed() < PROMPT, "{:?}", start.elapsed());
    let start = Instant::now();
    world.user_runs(&["stop", "beta"]);
    world.user_runs(&["resume", "beta"]);
    assert!(
        start.elapsed() < 2 * PROMPT,
        "hypr-computer stop and resume took {:?} while the service hangs",
        start.elapsed()
    );

    told_by_the_host(&world, &alpha);
    told_by_the_host(&world, &beta);
}

#[test]
fn the_host_desktop_shows_each_agents_notice_once() {
    let world = World::on_desktop(Service::Hung);
    let mut alpha = world.agent("alpha");
    told_by_the_host(&world, &alpha);

    for _ in 0..3 {
        alpha.assert_works();
    }
    thread::sleep(TOLD);

    let notices: Vec<HostNotice> = world
        .host_notices()
        .into_iter()
        .filter(|n| n.message.contains(&alpha.label()))
        .collect();
    assert_eq!(notices.len(), 1, "{notices:?}");
}

/// The service answers after all, once the host has shown its notice: its
/// notice, with the buttons, is kept, and works.
#[test]
fn a_late_answer_gives_the_agent_its_notice_and_its_stop_button() {
    let world = World::on_desktop(Service::Hung);
    let mut alpha = world.agent("alpha");
    let mut beta = world.agent("beta");
    told_by_the_host(&world, &alpha);

    world.notifications().release();
    let (id, notice) = world.notice_of(&alpha);
    assert_eq!(notice.expire_timeout, 0, "{notice:?}");
    assert!(
        !world.notifications().closes().contains(&id) && world.replacements_of(id).is_empty(),
        "the late notice must stay while the agent works"
    );

    world.notifications().press(id, "stop");
    alpha.assert_becomes_stopped();
    beta.assert_works();
}

/// ...and when the agent ends, the late notice is replaced by "done" as any
/// other: it doesn't outlive its session.
#[test]
fn a_late_answers_notice_is_replaced_by_done_when_the_agent_ends() {
    let world = World::on_desktop(Service::Hung);
    world.notifications().keep_closed();
    let mut alpha = world.agent("alpha");
    told_by_the_host(&world, &alpha);
    world.notifications().release();
    let (id, _) = world.notice_of(&alpha);

    alpha.exit();

    let done = world.wait_for_replacement_of(id);
    assert!(done.is_short_lived(), "{done:?}");
    assert!(
        world.notifications().on_screen()[&id].is_short_lived(),
        "alpha's notice outlives it"
    );
}

#[test]
fn a_service_that_answers_in_time_gets_no_host_notice() {
    let world = World::on_desktop(Service::Up);
    let mut alpha = world.agent("alpha");
    world.notice_of(&alpha);
    alpha.assert_works();

    thread::sleep(TOLD);

    assert!(
        world.host_notices().is_empty(),
        "the user was told twice: {:?}",
        world.host_notices()
    );
}

#[test]
fn without_a_bus_or_a_desktop_the_daemon_works_as_before() {
    let mut world = World::start(Service::NoBus);
    let mut alpha = world.agent("alpha");
    let mut beta = world.agent("beta");
    alpha.assert_works();
    beta.assert_works();

    world.user_runs(&["stop", "alpha"]);
    alpha.assert_becomes_stopped();
    beta.assert_works();
    world.user_runs(&["resume", "alpha"]);
    alpha.assert_works();

    alpha.exit();
    beta.assert_works();
    assert!(world.daemon.is_running(), "{}", world.daemon.log());
}

/// The daemon is stopped while the service still hangs: it exits all the
/// same (`terminate` fails the test if it doesn't within 10 s).
#[test]
fn the_daemon_stops_on_sigterm_while_the_service_hangs() {
    let mut world = World::on_desktop(Service::Hung);
    let mut alpha = world.agent("alpha");
    world.asked_for_notice_of(&alpha);
    alpha.assert_works();

    let start = Instant::now();
    world.daemon.terminate();
    assert!(
        start.elapsed() < Duration::from_secs(8),
        "{:?}",
        start.elapsed()
    );
}
