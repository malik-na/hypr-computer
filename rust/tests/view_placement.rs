//! Each agent session gets a live view on the user's workspace 10, placed
//! silently: opening it never takes the user's focus or moves them to another
//! workspace (#27). The desktop is a fake Hyprland and a host compositor.
#![forbid(unsafe_code)]

mod support;
mod view_support;

use std::thread;
use std::time::Duration;

use view_support::probe::Probe;
use view_support::{Desktop, USER_WINDOW, USER_WORKSPACE, VIEWS_WORKSPACE};

const GREY: [u8; 3] = [0x80, 0x80, 0x80];

/// Fails unless every view is on workspace 10 and the user was never moved
/// nor lost focus.
fn assert_placed_silently(desk: &Desktop, daemon: &support::Daemon, addresses: &[String]) {
    let (on, shown, moved, focused, refocused) = {
        let w = desk.hypr.world();
        let on: Vec<Option<String>> = addresses.iter().map(|a| w.workspace_of(a)).collect();
        (
            on,
            w.shown_workspace.clone(),
            w.workspace_changes.clone(),
            w.focused.clone(),
            w.focus_changes.clone(),
        )
    };
    let report = || desk.report(daemon);
    for (address, ws) in addresses.iter().zip(&on) {
        assert_eq!(
            ws.as_deref(),
            Some(VIEWS_WORKSPACE),
            "the view {address} isn't on workspace 10\n{}",
            report()
        );
    }
    assert_eq!(shown, USER_WORKSPACE, "{}", report());
    assert!(
        moved.is_empty(),
        "the user was moved to another workspace, if only for a moment: {moved:?}\n{}",
        report()
    );
    assert_eq!(focused.as_deref(), Some(USER_WINDOW), "{}", report());
    assert!(
        refocused.is_empty(),
        "the user's focus moved, if only for a moment: {refocused:?}\n{}",
        report()
    );
}

#[test]
fn an_agents_view_opens_on_workspace_10_and_the_user_keeps_their_focus_and_workspace() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();
    let _agent = desk.agent("placement-agent");

    let view = desk.wait_for_views(1, &daemon).remove(0);
    // Long enough for the view to draw, retitle itself or ask for focus.
    thread::sleep(Duration::from_secs(1));
    assert_placed_silently(&desk, &daemon, &[view.address.unwrap()]);
}

#[test]
fn a_second_agents_view_also_opens_silently_on_workspace_10() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();
    let _a = desk.agent("first-agent");
    desk.wait_for_views(1, &daemon);
    let _b = desk.agent("second-agent");
    let views = desk.wait_for_views(2, &daemon);
    thread::sleep(Duration::from_secs(1));
    let addresses: Vec<String> = views.into_iter().map(|v| v.address.unwrap()).collect();
    assert_placed_silently(&desk, &daemon, &addresses);
}

/// The rule that sends views to workspace 10 must not catch the user's own
/// windows.
#[test]
fn the_users_own_new_windows_still_open_where_the_user_is() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();
    let _agent = desk.agent("placement-agent");
    desk.wait_for_views(1, &daemon);

    let _mine = Probe::paint(desk.host.socket(), [GREY; 4], "org.user.Editor");
    let (mine, shown, focused) = {
        let w = desk.hypr.world();
        let mine = w
            .windows
            .iter()
            .find(|w| w.class == "org.user.Editor")
            .cloned();
        (mine, w.shown_workspace.clone(), w.focused.clone())
    };
    let mine =
        mine.unwrap_or_else(|| panic!("the user's window never mapped\n{}", desk.report(&daemon)));
    assert_eq!(mine.workspace, USER_WORKSPACE, "{}", desk.report(&daemon));
    assert_eq!(shown, USER_WORKSPACE);
    assert_eq!(focused.as_deref(), Some(mine.address.as_str()));
}
