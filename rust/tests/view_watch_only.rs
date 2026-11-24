//! The live view is watch-only: the user's pointer and keys on it never reach
//! the agent's apps, and change nothing on the agent's screen (#27), until
//! the user clicks into it to take it over (#30, tests/takeover_*_spec.rs).
#![forbid(unsafe_code)]

mod support;
mod view_support;

use std::thread;
use std::time::Duration;

use view_support::probe::{Probe, Quadrants};
use view_support::{Desktop, VIEWS_WORKSPACE, wait_until_view_shows};

const COLOURS: Quadrants = [
    [0xd0, 0x20, 0x20],
    [0x20, 0xb0, 0x30],
    [0x20, 0x40, 0xd0],
    [0xe0, 0xc0, 0x20],
];

#[test]
fn the_users_pointer_and_keys_on_the_view_never_reach_the_agents_app_without_a_click() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();
    let mut agent = desk.agent("watched-agent");
    let app = Probe::paint(&desk.screen(1), COLOURS, "agent.app");
    let address = desk.wait_for_views(1, &daemon).remove(0).address.unwrap();
    desk.hypr.user_shows_workspace(VIEWS_WORKSPACE);
    wait_until_view_shows(&desk, &mut agent, &address, Duration::from_secs(1))
        .unwrap_or_else(|e| panic!("{e}\n{}", desk.report(&daemon)));
    thread::sleep(Duration::from_millis(300));
    let before = agent.screenshot();
    let events_before = app.events();

    desk.host
        .user_input_without_clicking(&address)
        .unwrap_or_else(|e| panic!("{e}\n{}", desk.report(&daemon)));
    thread::sleep(Duration::from_millis(500));

    let after = agent.screenshot();
    let events = app.events();
    assert_eq!(
        events,
        events_before,
        "the agent's app got the user's input (its screen offers a seat: {})",
        app.has_seat()
    );
    for y in 0..before.height {
        for x in 0..before.width {
            assert_eq!(
                before.pixel(x, y),
                after.pixel(x, y),
                "the user's input changed the agent's screen at {x},{y}"
            );
        }
    }
    assert!(daemon.log().contains("session 1"), "{}", daemon.log());
    assert!(
        !daemon.log().contains("session 1 ended"),
        "the user's input ended the session:\n{}",
        daemon.log()
    );
}
