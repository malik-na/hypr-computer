//! Several agents' live views tile side by side on workspace 10, up to 4 by
//! default; a fifth agent's view is refused with a message naming the limit,
//! and that agent's session still works (#27).
#![forbid(unsafe_code)]

mod support;
mod view_support;

use std::collections::HashSet;
use std::thread;
use std::time::Duration;

use view_support::probe::Probe;
use view_support::{Desktop, USER_WORKSPACE, VIEWS_WORKSPACE, numbers};

const LIMIT: usize = 4;
const TEAL: [u8; 3] = [0x20, 0xc0, 0xc0];

#[test]
fn four_agents_views_tile_together_on_workspace_10() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();
    let mut agents = Vec::new();
    for n in 1..=LIMIT {
        agents.push(desk.agent(&format!("tiling-agent-{n}")));
        desk.wait_for_views(n, &daemon);
    }
    thread::sleep(Duration::from_millis(500));

    let views = desk.wait_for_views(LIMIT, &daemon);
    let addresses: HashSet<String> = views.iter().map(|v| v.address.clone().unwrap()).collect();
    assert_eq!(addresses.len(), LIMIT, "{views:?}");
    let world = desk.hypr.world();
    for address in &addresses {
        let window = world.find(address).unwrap();
        assert_eq!(window.workspace, VIEWS_WORKSPACE, "{window:?}");
        assert!(!window.floating, "views tile, they don't float: {window:?}");
    }
    assert_eq!(world.shown_workspace, USER_WORKSPACE);
}

#[test]
fn a_fifth_agents_view_is_refused_with_a_message_naming_the_limit_and_its_session_still_works() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();
    let mut agents = Vec::new();
    for n in 1..=LIMIT {
        agents.push(desk.agent(&format!("tiling-agent-{n}")));
        desk.wait_for_views(n, &daemon);
    }
    let before = desk.messages(&daemon).len();

    let mut fifth = desk.agent("tiling-agent-5");
    // Its session works: its screen shows its app.
    let _app = Probe::paint(&desk.screen(5), [TEAL; 4], "agent.app");
    assert_eq!(fifth.screenshot().pixel(640, 400), TEAL);
    thread::sleep(Duration::from_secs(2));

    let views = desk.mapped_views();
    assert_eq!(
        views.len(),
        LIMIT,
        "a fifth view opened: {views:?}\n{}",
        desk.report(&daemon)
    );
    assert!(
        !views.iter().any(|v| v.title.contains("tiling-agent-5")),
        "{views:?}"
    );
    let messages = desk.messages(&daemon);
    let named = messages[before..].iter().find(|m| {
        let lower = m.to_lowercase();
        numbers(m).contains(&LIMIT.to_string())
            && ["view", "watch", "limit", "most", "max"]
                .iter()
                .any(|w| lower.contains(w))
    });
    assert!(
        named.is_some(),
        "no message names the limit of {LIMIT} views; new messages: {:#?}",
        &messages[before..]
    );
    assert_eq!(fifth.screenshot().width, 1280);
    assert!(
        !daemon.log().contains("session 5 ended"),
        "{}",
        daemon.log()
    );
}
