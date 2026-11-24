//! The user watches agents live: each session's screen is a window on the
//! user's desktop, on workspace 10. The desktop is a fake one: a compositor
//! for the views and a fake Hyprland that records what it is asked.
#![forbid(unsafe_code)]

mod support;

use std::thread;
use std::time::{Duration, Instant};

use support::app::App;
use support::desktop::VIEW_APP_ID;
use support::hyprland::PLACE_VIEWS;
use support::{Daemon, Desktop, Env, Image, Shim, TIMEOUT};

const RED: [u8; 3] = [0xd0, 0x20, 0x20];
const BLUE: [u8; 3] = [0x20, 0x40, 0xd0];
const GREEN: [u8; 3] = [0x20, 0xc0, 0x40];

/// Waits until the desktop shows exactly `shot`, the agent's screenshot.
fn assert_desktop_shows(desktop: &mut Desktop, shot: &Image) {
    let end = Instant::now() + TIMEOUT;
    loop {
        let unlike = desktop.screenshot().pixels_unlike(shot);
        if unlike == 0 {
            return;
        }
        assert!(
            Instant::now() < end,
            "the desktop still shows something else: {unlike} pixels differ from the agent's screenshot"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// An agent whose screen is ready.
fn agent(env: &Env) -> Shim {
    let mut shim = Shim::start(env);
    shim.initialize();
    shim.screenshot();
    shim
}

#[test]
fn an_agents_screen_opens_on_the_desktop_as_a_window_of_its_own() {
    let env = Env::new();
    let mut desktop = Desktop::start(&env);
    let _daemon = Daemon::start_on(&env, &desktop);
    let _agent = agent(&env);

    let views = desktop.wait_for_views(1);
    assert_eq!(views[0].app_id, VIEW_APP_ID);
    assert_eq!(views[0].title, "hypr-computer: session 1");
}

#[test]
fn the_view_shows_what_a_screenshot_shows() {
    let env = Env::new();
    let mut desktop = Desktop::start(&env);
    let _daemon = Daemon::start_on(&env, &desktop);
    let mut shim = agent(&env);
    desktop.wait_for_views(1);

    let _app = App::paint(&env.screen_socket(1), RED, BLUE);
    let shot = shim.screenshot();
    assert_eq!(shot.pixel(0, 0), RED);
    assert_desktop_shows(&mut desktop, &shot);
}

#[test]
fn the_view_is_current_again_right_after_being_hidden() {
    let env = Env::new();
    let mut desktop = Desktop::start(&env);
    let _daemon = Daemon::start_on(&env, &desktop);
    let mut shim = agent(&env);
    let first = App::paint(&env.screen_socket(1), RED, BLUE);
    desktop.wait_for_views(1);
    assert_desktop_shows(&mut desktop, &shim.screenshot());

    // A window of the user's hides the view while the agent's screen changes.
    let cover = App::paint(desktop.socket(), GREEN, GREEN);
    assert_eq!(desktop.screenshot().pixel(640, 400), GREEN);
    drop(first);
    let _second = App::paint(&env.screen_socket(1), BLUE, RED);
    let shot = shim.screenshot();
    assert_eq!(shot.pixel(0, 0), BLUE);

    drop(cover);
    assert_desktop_shows(&mut desktop, &shot);
}

#[test]
fn the_view_opens_on_workspace_10_only_once_hyprland_will_keep_it_out_of_the_users_way() {
    let env = Env::new();
    let mut desktop = Desktop::start(&env);
    desktop.hyprland.hold();
    let _daemon = Daemon::start_on(&env, &desktop);
    let mut shim = Shim::start(&env);
    shim.initialize();

    assert_eq!(desktop.hyprland.wait_for_requests(1), [PLACE_VIEWS]);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        desktop.views(),
        [],
        "the view opened before Hyprland placed it"
    );

    desktop.hyprland.release();
    desktop.wait_for_views(1);
    // Nothing else: the user's workspace and focus are left alone.
    assert_eq!(desktop.hyprland.requests(), [PLACE_VIEWS]);
}

#[test]
fn the_view_closes_when_its_session_ends() {
    let env = Env::new();
    let mut desktop = Desktop::start(&env);
    let daemon = Daemon::start_on(&env, &desktop);
    let mut first = agent(&env);
    let _second = agent(&env);
    desktop.wait_for_views(2);

    first.kill();
    daemon.wait_for_log("session 1 ended");
    let left = desktop.wait_for_views(1);
    assert_eq!(left[0].title, "hypr-computer: session 2");
}

#[test]
fn off_hyprland_agents_still_work_and_the_daemon_says_why_there_is_no_view() {
    let env = Env::new();
    let daemon = Daemon::start(&env);
    let mut shim = agent(&env);
    daemon.wait_for_log(
        "session 1: no live view: there is no desktop to show it on: WAYLAND_DISPLAY isn't set",
    );
    assert_eq!(shim.screenshot().width, 1280);
}
