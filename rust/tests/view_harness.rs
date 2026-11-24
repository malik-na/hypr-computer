//! Checks on the fake desktop the live-view tests use (`view_support`): that
//! the fake Hyprland places windows and moves focus as Hyprland does, and
//! that the host compositor shows, hides and feeds input to windows as the
//! tests assume. If these fail, the view tests' verdicts mean nothing.
#![forbid(unsafe_code)]

mod support;
mod view_support;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::{Duration, Instant};

use view_support::probe::{Probe, Quadrants};
use view_support::{Desktop, USER_WINDOW, USER_WORKSPACE};

const COLOURS: Quadrants = [
    [0xd0, 0x20, 0x20],
    [0x20, 0xb0, 0x30],
    [0x20, 0x40, 0xd0],
    [0xe0, 0xc0, 0x20],
];

/// One request to the fake Hyprland, as hyprctl sends it.
fn hyprctl(desk: &Desktop, request: &str) -> String {
    let socket = desk.env.runtime_dir().join("hypr/fakehypr/.socket.sock");
    let mut conn = UnixStream::connect(socket).unwrap();
    conn.write_all(request.as_bytes()).unwrap();
    let mut reply = String::new();
    conn.read_to_string(&mut reply).unwrap();
    reply
}

fn wait_for_window(desk: &Desktop, app_id: &str) -> String {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let found = desk
            .host
            .windows()
            .into_iter()
            .find(|w| w.app_id == app_id && w.address.is_some());
        if let Some(w) = found {
            return w.address.unwrap();
        }
        assert!(Instant::now() < end, "{app_id} never mapped");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_window_without_rules_opens_where_the_user_is_and_takes_focus() {
    let desk = Desktop::new();
    let _w = Probe::paint(desk.host.socket(), COLOURS, "plain.app");
    let address = wait_for_window(&desk, "plain.app");
    let world = desk.hypr.world();
    assert_eq!(
        world.workspace_of(&address).as_deref(),
        Some(USER_WORKSPACE)
    );
    assert_eq!(world.focused.as_deref(), Some(address.as_str()));
}

#[test]
fn a_silent_workspace_rule_places_a_window_without_disturbing_the_user() {
    let desk = Desktop::new();
    let reply = hyprctl(
        &desk,
        r#"/eval hl.window_rule({ name = "views", match = { class = "^(quiet\\.app)$" }, workspace = "10 silent" })"#,
    );
    assert_eq!(reply, "ok");
    let _w = Probe::paint(desk.host.socket(), COLOURS, "quiet.app");
    let address = wait_for_window(&desk, "quiet.app");
    let world = desk.hypr.world();
    assert_eq!(world.workspace_of(&address).as_deref(), Some("10"));
    assert_eq!(world.shown_workspace, USER_WORKSPACE);
    assert_eq!(world.focused.as_deref(), Some(USER_WINDOW));
    assert!(world.workspace_changes.is_empty() && world.focus_changes.is_empty());
}

#[test]
fn a_loud_workspace_rule_takes_the_user_there() {
    let desk = Desktop::new();
    hyprctl(
        &desk,
        r#"/eval hl.window_rule({ name = "loud", match = { class = "loud.app" }, workspace = "10" })"#,
    );
    let _w = Probe::paint(desk.host.socket(), COLOURS, "loud.app");
    let address = wait_for_window(&desk, "loud.app");
    let world = desk.hypr.world();
    assert_eq!(world.shown_workspace, "10");
    assert_eq!(world.focused.as_deref(), Some(address.as_str()));
}

#[test]
fn dispatching_a_workspace_shows_it_and_answers_queries() {
    let desk = Desktop::new();
    assert_eq!(
        hyprctl(&desk, "/dispatch hl.dsp.focus({ workspace = 10 })"),
        "ok"
    );
    assert_eq!(desk.hypr.world().shown_workspace, "10");
    let active: serde_json::Value =
        serde_json::from_str(&hyprctl(&desk, "j/activeworkspace")).unwrap();
    assert_eq!(active["id"], 10);
    let clients: serde_json::Value = serde_json::from_str(&hyprctl(&desk, "j/clients")).unwrap();
    assert_eq!(clients[0]["address"], USER_WINDOW);
}

#[test]
fn the_host_shows_what_a_window_draws_and_hides_it_with_its_workspace() {
    let desk = Desktop::new();
    let _w = Probe::paint(desk.host.socket(), COLOURS, "plain.app");
    let address = wait_for_window(&desk, "plain.app");
    let picture = desk.host.capture(&address).unwrap();
    assert_eq!((picture.width, picture.height), (1280, 800));
    assert_eq!(picture.pixel(100, 100), COLOURS[0]);
    assert_eq!(picture.pixel(1200, 100), COLOURS[1]);
    assert_eq!(picture.pixel(100, 700), COLOURS[2]);
    assert_eq!(picture.pixel(1200, 700), COLOURS[3]);

    let suspended = |desk: &Desktop| {
        desk.host
            .windows()
            .into_iter()
            .find(|w| w.address.as_deref() == Some(address.as_str()))
            .unwrap()
            .suspended
    };
    desk.hypr.user_shows_workspace("5");
    thread::sleep(Duration::from_millis(100));
    assert!(suspended(&desk));
    desk.hypr.user_shows_workspace(USER_WORKSPACE);
    thread::sleep(Duration::from_millis(100));
    assert!(!suspended(&desk));
}

#[test]
fn the_users_input_reaches_a_window_on_the_host() {
    let desk = Desktop::new();
    let w = Probe::paint(desk.host.socket(), COLOURS, "plain.app");
    let address = wait_for_window(&desk, "plain.app");
    desk.host.user_input(&address).unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    while !w.events().iter().any(|e| e.starts_with("key")) {
        assert!(Instant::now() < end, "got {:?}", w.events());
        thread::sleep(Duration::from_millis(10));
    }
    let events = w.events();
    assert!(events.iter().any(|e| e.contains("Button")), "{events:?}");
    assert!(events.iter().any(|e| e.contains("Motion")), "{events:?}");
}
