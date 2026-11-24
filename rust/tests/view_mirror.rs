//! The live view shows what a screenshot of the agent's screen shows, live,
//! and also right after it was hidden (#27). Hidden means the user is on
//! another workspace: like Hyprland, the host then sends the view no frame
//! callbacks and tells it it is suspended.
#![forbid(unsafe_code)]

mod support;
mod view_support;

use std::time::Duration;

use view_support::probe::{Probe, Quadrants};
use view_support::{Desktop, USER_WORKSPACE, VIEWS_WORKSPACE, wait_until_view_shows};

const FIRST: Quadrants = [
    [0xd0, 0x20, 0x20],
    [0x20, 0xb0, 0x30],
    [0x20, 0x40, 0xd0],
    [0xe0, 0xc0, 0x20],
];
const SECOND: Quadrants = [
    [0x20, 0xc0, 0xc0],
    [0x90, 0x30, 0xa0],
    [0xf0, 0xf0, 0xf0],
    [0x60, 0x30, 0x10],
];

/// How soon a view must catch up with the screen.
const PROMPTLY: Duration = Duration::from_secs(1);

#[test]
fn the_view_shows_the_agents_screen_when_the_user_first_looks_and_follows_it_live() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();
    let mut agent = desk.agent("mirror-agent");
    let _app = Probe::paint(&desk.screen(1), FIRST, "agent.app");
    let view = desk.wait_for_views(1, &daemon).remove(0);
    let address = view.address.unwrap();

    // The view opened hidden; the user now looks at it for the first time.
    desk.hypr.user_shows_workspace(VIEWS_WORKSPACE);
    wait_until_view_shows(&desk, &mut agent, &address, PROMPTLY)
        .unwrap_or_else(|e| panic!("on first look: {e}\n{}", desk.report(&daemon)));

    let _next = Probe::paint(&desk.screen(1), SECOND, "agent.app");
    wait_until_view_shows(&desk, &mut agent, &address, PROMPTLY)
        .unwrap_or_else(|e| panic!("after the screen changed: {e}\n{}", desk.report(&daemon)));
}

#[test]
fn right_after_being_hidden_the_view_shows_the_screen_as_it_is_now() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();
    let mut agent = desk.agent("mirror-agent");
    let _app = Probe::paint(&desk.screen(1), FIRST, "agent.app");
    let view = desk.wait_for_views(1, &daemon).remove(0);
    let address = view.address.unwrap();
    desk.hypr.user_shows_workspace(VIEWS_WORKSPACE);
    wait_until_view_shows(&desk, &mut agent, &address, PROMPTLY)
        .unwrap_or_else(|e| panic!("before hiding: {e}\n{}", desk.report(&daemon)));

    // The user goes back to work; the agent's screen changes meanwhile.
    desk.hypr.user_shows_workspace(USER_WORKSPACE);
    let _next = Probe::paint(&desk.screen(1), SECOND, "agent.app");
    let shot = agent.screenshot();
    assert_eq!(
        shot.pixel(100, 100),
        SECOND[0],
        "the screenshot isn't current while the view is hidden"
    );

    desk.hypr.user_shows_workspace(VIEWS_WORKSPACE);
    let took = wait_until_view_shows(&desk, &mut agent, &address, PROMPTLY)
        .unwrap_or_else(|e| panic!("after being hidden: {e}\n{}", desk.report(&daemon)));
    eprintln!("the view caught up {took:?} after being shown again");
}

#[test]
fn a_view_hidden_and_shown_many_times_still_follows_the_screen() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();
    let mut agent = desk.agent("mirror-agent");
    let _app = Probe::paint(&desk.screen(1), FIRST, "agent.app");
    let address = desk.wait_for_views(1, &daemon).remove(0).address.unwrap();

    let mut apps = Vec::new();
    for round in 0..4 {
        desk.hypr.user_shows_workspace(USER_WORKSPACE);
        let colours = if round % 2 == 0 { SECOND } else { FIRST };
        apps.push(Probe::paint(&desk.screen(1), colours, "agent.app"));
        desk.hypr.user_shows_workspace(VIEWS_WORKSPACE);
        wait_until_view_shows(&desk, &mut agent, &address, PROMPTLY)
            .unwrap_or_else(|e| panic!("round {round}: {e}\n{}", desk.report(&daemon)));
    }
}
