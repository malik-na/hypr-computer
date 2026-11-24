//! `hypr-computer watch` takes the user to workspace 10, where the live views
//! are, and says something useful when no agent is running (#27).
#![forbid(unsafe_code)]

mod support;
mod view_support;

use std::process::Output;
use std::thread;
use std::time::Duration;

use view_support::{Desktop, VIEWS_WORKSPACE, wait_until_view_shows};

fn printed(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn watch_takes_the_user_to_workspace_10_where_the_agents_view_shows_its_screen() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();
    let mut agent = desk.agent("watched-agent");
    let address = desk.wait_for_views(1, &daemon).remove(0).address.unwrap();
    assert_ne!(desk.hypr.world().shown_workspace, VIEWS_WORKSPACE);

    let out = desk.hypr_computer(&["watch"]);
    assert!(
        out.status.success(),
        "{}\n{}",
        printed(&out),
        desk.report(&daemon)
    );
    let shown = desk.hypr.world().shown_workspace.clone();
    assert_eq!(
        shown,
        VIEWS_WORKSPACE,
        "{}\n{}",
        printed(&out),
        desk.report(&daemon)
    );
    wait_until_view_shows(&desk, &mut agent, &address, Duration::from_secs(1))
        .unwrap_or_else(|e| panic!("{e}\n{}", desk.report(&daemon)));

    // Watching is not an agent: no session, no extra view.
    thread::sleep(Duration::from_millis(300));
    assert_eq!(desk.mapped_views().len(), 1, "{}", desk.report(&daemon));
    assert!(
        !daemon.log().contains("session 2"),
        "`watch` opened a session of its own:\n{}",
        daemon.log()
    );
}

#[test]
fn watch_says_so_when_no_agent_is_running() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();

    let out = desk.hypr_computer(&["watch"]);
    let text = printed(&out);
    assert!(
        text.to_lowercase().contains("agent"),
        "`hypr-computer watch` with no agent running should say so; it printed {text:?}\n{}",
        desk.report(&daemon)
    );
    assert!(!text.contains("panicked"), "{text}");
    assert!(!daemon.log().contains("session 1"), "{}", daemon.log());
}

#[test]
fn watch_says_something_useful_when_the_daemon_isnt_running() {
    let desk = Desktop::new();

    let out = desk.hypr_computer(&["watch"]);
    let text = printed(&out).to_lowercase();
    assert!(
        text.contains("agent") || text.contains("daemon"),
        "`hypr-computer watch` with nothing running should say why there's nothing to watch; it printed {text:?}"
    );
    assert!(!text.contains("panicked"), "{text}");
}
