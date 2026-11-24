//! Each live view is labelled with its agent's name (the MCP clientInfo
//! name) and its session number, so the user can tell agents apart (#27).
//! The label is the view window's title, which the user's bar and window
//! list show.
#![forbid(unsafe_code)]

mod support;
mod view_support;

use std::thread;
use std::time::Duration;

use view_support::{Desktop, numbers};

#[test]
fn each_view_is_titled_with_its_agents_name_and_session_number() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();
    let _a = desk.agent("alpha-tester");
    desk.wait_for_views(1, &daemon);
    let _b = desk.agent("beta-tester");
    desk.wait_for_views(2, &daemon);
    // A title set after the window opened counts too.
    thread::sleep(Duration::from_millis(500));

    let views = desk.mapped_views();
    for (name, session) in [("alpha-tester", "1"), ("beta-tester", "2")] {
        let view = views
            .iter()
            .find(|v| v.title.contains(name))
            .unwrap_or_else(|| {
                panic!(
                    "no view's title names {name:?}: {views:?}\n{}",
                    desk.report(&daemon)
                )
            });
        assert!(
            numbers(&view.title).contains(&session.to_owned()),
            "{name}'s view should name session {session}: {:?}",
            view.title
        );
        let address = view.address.as_deref().unwrap();
        let listed = desk.hypr.world().find(address).unwrap();
        assert_eq!(
            listed.title, view.title,
            "Hyprland lists the view under another title"
        );
    }
}
