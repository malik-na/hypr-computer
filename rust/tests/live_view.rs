//! The user watches agents live: each session's screen is a window on the
//! user's desktop, on workspace 10. The desktop is a fake one: a compositor
//! for the views and a fake Hyprland that records what it is asked.
#![forbid(unsafe_code)]

mod support;

use std::fs;
use std::os::unix::net::UnixListener;
use std::thread;
use std::time::{Duration, Instant};

use support::app::App;
use support::desktop::VIEW_APP_ID;
use support::hyprland::{Hyprland, PLACE_VIEWS, SIGNATURE};
use support::{Daemon, Desktop, Env, Image, Shim, TIMEOUT, daemon_bin};

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

/// Agents `first..=last`, called `agent-<n>`, each with its screen ready.
fn agents(env: &Env, first: u64, last: u64) -> Vec<Shim> {
    (first..=last)
        .map(|n| {
            let mut shim = Shim::start(env);
            shim.initialize_as(&format!("agent-{n}"));
            shim.screenshot();
            shim
        })
        .collect()
}

#[test]
fn an_agents_screen_opens_on_the_desktop_as_a_window_named_after_the_agent() {
    let env = Env::new();
    let mut desktop = Desktop::start(&env);
    let _daemon = Daemon::start_on(&env, &desktop);
    let mut shim = Shim::start(&env);
    shim.initialize_as("claude-code");

    let views = desktop.wait_for_views(1);
    assert_eq!(views[0].app_id, VIEW_APP_ID);
    assert_eq!(views[0].title, "claude-code (session 1)");
}

#[test]
fn the_view_waits_for_the_agent_to_say_its_name() {
    let env = Env::new();
    let mut desktop = Desktop::start(&env);
    let daemon = Daemon::start_on(&env, &desktop);
    let mut shim = Shim::start(&env);
    daemon.wait_for_log("session 1: screen ready");
    thread::sleep(Duration::from_millis(300));
    assert_eq!(desktop.views(), []);

    shim.initialize_as("codex");
    assert_eq!(desktop.wait_for_views(1)[0].title, "codex (session 1)");
}

/// The `_meta` a client on MCP 2026-07-28 sends with every request, in place
/// of an `initialize` handshake; with its name if `name` is given.
fn inline_meta(name: Option<&str>) -> serde_json::Value {
    let mut meta = serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    if let Some(name) = name {
        meta["io.modelcontextprotocol/clientInfo"] =
            serde_json::json!({"name": name, "version": "2"});
    }
    serde_json::json!({ "_meta": meta })
}

#[test]
fn an_agent_without_a_handshake_is_named_from_its_requests() {
    let env = Env::new();
    let mut desktop = Desktop::start(&env);
    let _daemon = Daemon::start_on(&env, &desktop);
    let mut shim = Shim::start(&env);
    let tools = shim.request("tools/list", inline_meta(Some("claude-code")));
    assert!(tools.get("error").is_none(), "{tools}");

    assert_eq!(
        desktop.wait_for_views(1)[0].title,
        "claude-code (session 1)"
    );
}

#[test]
fn an_agent_that_gives_no_name_is_shown_as_an_agent() {
    let env = Env::new();
    let mut desktop = Desktop::start(&env);
    let _daemon = Daemon::start_on(&env, &desktop);
    let mut shim = Shim::start(&env);
    let tools = shim.request("tools/list", inline_meta(None));
    assert!(tools.get("error").is_none(), "{tools}");

    assert_eq!(desktop.wait_for_views(1)[0].title, "agent (session 1)");
}

#[test]
fn several_agents_views_are_on_the_desktop_at_once_each_showing_its_own_screen() {
    let env = Env::new();
    let mut desktop = Desktop::start(&env);
    let _daemon = Daemon::start_on(&env, &desktop);
    let mut both = agents(&env, 1, 2);
    let _on_1 = App::paint(&env.screen_socket(1), RED, RED);
    let _on_2 = App::paint(&env.screen_socket(2), BLUE, BLUE);

    let titles: Vec<String> = desktop
        .wait_for_views(2)
        .into_iter()
        .map(|v| v.title)
        .collect();
    assert_eq!(titles, ["agent-1 (session 1)", "agent-2 (session 2)"]);
    // The fake desktop shows its newest window: agent 2's view.
    let shot = both[1].screenshot();
    assert_eq!(shot.pixel(640, 400), BLUE);
    assert_desktop_shows(&mut desktop, &shot);
    assert_eq!(both[0].screenshot().pixel(640, 400), RED);
}

#[test]
fn the_view_shows_what_a_screenshot_shows() {
    let env = Env::new();
    let mut desktop = Desktop::start(&env);
    let _daemon = Daemon::start_on(&env, &desktop);
    let mut shim = agent(&env);
    desktop.wait_for_views(1);

    let _app = App::paint(&env, 1, RED, BLUE);
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
    let first = App::paint(&env, 1, RED, BLUE);
    desktop.wait_for_views(1);
    assert_desktop_shows(&mut desktop, &shim.screenshot());

    // A window of the user's hides the view while the agent's screen changes.
    let cover = App::paint_on(&env, desktop.socket(), GREEN, GREEN);
    assert_eq!(desktop.screenshot().pixel(640, 400), GREEN);
    drop(first);
    let _second = App::paint(&env, 1, BLUE, RED);
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
fn a_view_hyprland_refuses_to_place_never_opens() {
    let env = Env::new();
    let mut desktop = Desktop::start(&env);
    desktop.hyprland.answer_with("error: no such rule field");
    let daemon = Daemon::start_on(&env, &desktop);
    let mut shim = agent(&env);

    daemon.wait_for_log("session 1: no live view: Hyprland refused");
    thread::sleep(Duration::from_millis(300));
    assert_eq!(desktop.views(), [], "it opened where Hyprland put it");
    assert_eq!(shim.screenshot().width, 1280);
}

#[test]
fn the_view_closes_when_its_session_ends() {
    let env = Env::new();
    let mut desktop = Desktop::start(&env);
    let daemon = Daemon::start_on(&env, &desktop);
    let mut both = agents(&env, 1, 2);
    desktop.wait_for_views(2);

    both[0].kill();
    daemon.wait_for_log("session 1 ended");
    let left = desktop.wait_for_views(1);
    assert_eq!(left[0].title, "agent-2 (session 2)");
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

#[test]
fn a_desktop_that_never_answers_costs_the_agent_nothing() {
    let env = Env::new();
    let dir = env.path("stuck");
    fs::create_dir(&dir).unwrap();
    // Takes connections and never reads them, like a desktop that hangs.
    let _stuck = UnixListener::bind(dir.join("wayland-0")).unwrap();
    let _hyprland = Hyprland::start(&dir);
    let mut cmd = env.command(&daemon_bin());
    cmd.env("WAYLAND_DISPLAY", dir.join("wayland-0"))
        .env("HYPRLAND_INSTANCE_SIGNATURE", SIGNATURE);
    let daemon = Daemon::spawn(&env, cmd, "daemon.log");
    let mut shim = agent(&env);

    let log = daemon.wait_for_log("session 1: no live view");
    assert!(log.contains("didn't answer within 2s"), "{log}");
    assert_eq!(shim.screenshot().width, 1280);
    assert!(
        !daemon.log().contains("session 1 ended"),
        "{}",
        daemon.log()
    );
}
