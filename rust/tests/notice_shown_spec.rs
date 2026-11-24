//! #29: each working agent gets one desktop notification, naming it and
//! saying where to watch it, which stays until the user answers it. Written
//! from the spec (specs/Notice.tla and the issue) alone; spec29/ says what
//! interface it assumes.
#![forbid(unsafe_code)]

mod spec29;
mod support;

use std::thread;
use std::time::Duration;

use spec29::{Service, World, is_notice, says_where_to_watch};

#[test]
fn an_agent_that_connects_is_shown_one_notice_naming_it_and_saying_where_to_watch_it() {
    let world = World::start(Service::Up);
    let alpha = world.agent("alpha");

    let (id, notice) = world.notice_of(&alpha);

    assert_eq!(notice.app_name, "hypr-computer", "{notice:?}");
    assert_eq!(notice.replaces_id, 0, "{notice:?}");
    let text = notice.text();
    assert!(
        text.contains("alpha (session 1)"),
        "the notice must name the agent as its live view does: {text:?}"
    );
    assert!(
        says_where_to_watch(&text),
        "the notice must say where to watch the agent (Super+0 or `hypr-computer watch`): {text:?}"
    );
    assert_eq!(
        notice.expire_timeout, 0,
        "the notice must stay until the user answers it: {notice:?}"
    );
    let keys = notice.action_keys();
    assert!(
        keys.contains(&"ok") && keys.contains(&"stop"),
        "the notice needs the actions `ok` and `stop`: {notice:?}"
    );
    assert!(
        notice.label("ok").is_some_and(|l| l.contains("OK")),
        "{notice:?}"
    );
    assert!(
        notice.label("stop").is_some_and(|l| l.contains("Stop")),
        "{notice:?}"
    );
    assert!(
        world.notifications().on_screen().contains_key(&id),
        "the notice isn't on screen"
    );
}

#[test]
fn the_notice_stays_while_the_agent_works_and_is_not_shown_again() {
    let world = World::start(Service::Up);
    let mut alpha = world.agent("alpha");
    let (id, _) = world.notice_of(&alpha);

    for _ in 0..3 {
        alpha.assert_works();
    }
    thread::sleep(Duration::from_secs(1));
    alpha.assert_works();

    let notifies = world.notifications().notifies();
    assert_eq!(
        notifies.len(),
        1,
        "one agent at work must get one notice, and nothing else: {notifies:#?}"
    );
    assert!(
        world.notifications().closes().is_empty(),
        "the notice was closed while the agent works: {:?}",
        world.notifications().calls()
    );
    assert!(world.notifications().on_screen().contains_key(&id));
}

#[test]
fn each_agent_gets_a_notice_of_its_own() {
    let world = World::start(Service::Up);
    let alpha = world.agent("alpha");
    let beta = world.agent("beta");

    let (alpha_id, alpha_notice) = world.notice_of(&alpha);
    let (beta_id, beta_notice) = world.notice_of(&beta);

    assert_ne!(alpha_id, beta_id);
    assert!(
        !alpha_notice.text().contains("beta"),
        "{:?}",
        alpha_notice.text()
    );
    assert!(
        !beta_notice.text().contains("alpha"),
        "{:?}",
        beta_notice.text()
    );
    assert_eq!(world.notices_of(&alpha).len(), 1);
    assert_eq!(world.notices_of(&beta).len(), 1);
}

/// Two agents of the same kind have the same name; the session number tells
/// their notices apart, as it does their live views.
#[test]
fn two_agents_of_the_same_name_have_notices_the_user_can_tell_apart() {
    let world = World::start(Service::Up);
    let first = world.agent("claude-code");
    let second = world.agent("claude-code");

    let (first_id, first_notice) = world.notice_of(&first);
    let (second_id, second_notice) = world.notice_of(&second);

    assert_ne!(first_id, second_id);
    assert!(first_notice.text().contains("claude-code (session 1)"));
    assert!(second_notice.text().contains("claude-code (session 2)"));
    assert!(
        !first_notice.text().contains("(session 2)"),
        "{first_notice:?}"
    );
}

#[test]
fn the_notice_names_the_agent_so_it_waits_for_the_agent_to_say_its_name() {
    let world = World::start(Service::Up);
    let mut agent = world.connect();

    thread::sleep(Duration::from_secs(1));
    let early = world.notifications().notifies();
    assert!(
        early.iter().all(|n| !is_notice(n)),
        "a notice was asked for before the agent said its name: {early:#?}"
    );

    agent.introduce("gamma");
    let (_, notice) = world.notice_of(&agent);
    assert!(notice.text().contains("gamma (session 1)"), "{notice:?}");
}

/// A daemon started by systemd may have no `$DBUS_SESSION_BUS_ADDRESS`: the
/// session bus is then `$XDG_RUNTIME_DIR/bus`, where systemd puts it.
#[test]
fn without_a_bus_address_the_notice_goes_to_the_session_bus_in_the_runtime_dir() {
    let world = World::start(Service::InRuntimeDir);
    let alpha = world.agent("alpha");

    let (_, notice) = world.notice_of(&alpha);
    assert!(notice.text().contains("alpha (session 1)"), "{notice:?}");
}
