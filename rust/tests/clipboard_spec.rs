//! #30: the user's clipboard is shared with an agent's screen only while the
//! user has taken it over. Outside a take-over an app on the agent's screen
//! never gets what the user copied, and what the agent's apps copy never
//! reaches the user's clipboard. The interface these tests assume is at the
//! top of tests/spec30/mod.rs.
#![forbid(unsafe_code)]

mod spec30;
mod support;
mod view_support;

use std::thread;
use std::time::Duration;

use serde_json::json;

use spec30::clip::Clip;
use spec30::{TIMEOUT, World, wait_until};

/// Long enough for a clipboard change to have gone wherever it goes.
const SETTLE: Duration = Duration::from_millis(400);

#[test]
fn the_users_clipboard_reaches_the_agents_screen_only_during_a_take_over() {
    let mut w = World::new();
    let _agent = w.bare_agent("clip-agent");
    let clip = Clip::start(&w.desk.screen(1));

    w.desk.host.copy("user-secret-1");
    thread::sleep(SETTLE);
    assert_ne!(
        clip.paste().as_deref(),
        Some("user-secret-1"),
        "before any take-over, an app on the agent's screen got what the user copied"
    );

    w.user_clicks(1, 640.0, 400.0);
    assert!(
        wait_until(TIMEOUT, || clip.paste().as_deref() == Some("user-secret-1")),
        "during a take-over a paste on the agent's screen must get what the user copied; it got \
         {:?}\n{}",
        clip.paste(),
        w.report()
    );
    w.desk.host.copy("user-code-2");
    assert!(
        wait_until(TIMEOUT, || clip.paste().as_deref() == Some("user-code-2")),
        "during a take-over, what the user copies next must reach the agent's screen too; it got \
         {:?}",
        clip.paste()
    );

    w.user_gives();
    for _ in 0..2 {
        let got = clip.paste();
        assert!(
            !matches!(got.as_deref(), Some("user-secret-1" | "user-code-2")),
            "after the hand-back, an app on the agent's screen still got the user's clipboard: \
             {got:?}"
        );
        thread::sleep(SETTLE);
    }
    assert_eq!(
        w.desk.host.paste().as_deref(),
        Some("user-code-2"),
        "handing back must leave the user's own clipboard as it was"
    );
    w.desk.host.copy("user-secret-3");
    thread::sleep(SETTLE);
    assert_ne!(
        clip.paste().as_deref(),
        Some("user-secret-3"),
        "after the hand-back, what the user copied reached the agent's screen"
    );
}

#[test]
fn what_the_agents_apps_copy_reaches_the_user_only_during_a_take_over() {
    let mut w = World::new();
    let _agent = w.bare_agent("clip-agent");
    let clip = Clip::start(&w.desk.screen(1));
    w.desk.host.copy("user-own");

    clip.copy("agent-before");
    thread::sleep(SETTLE);
    assert_eq!(
        w.desk.host.paste().as_deref(),
        Some("user-own"),
        "outside a take-over, what an app on the agent's screen copied reached the user"
    );

    // The take-over starts: the screen gets the user's clipboard, and the
    // agent's earlier copy doesn't reach the user.
    w.user_clicks(1, 640.0, 400.0);
    assert!(
        wait_until(TIMEOUT, || clip.paste().as_deref() == Some("user-own")),
        "when a take-over starts, the agent's screen must get the user's clipboard; it has {:?}\n{}",
        clip.paste(),
        w.report()
    );
    assert_eq!(
        w.desk.host.paste().as_deref(),
        Some("user-own"),
        "the take-over gave the user what the agent's app had copied before it"
    );

    clip.copy("agent-during");
    assert!(
        wait_until(TIMEOUT, || w.desk.host.paste().as_deref()
            == Some("agent-during")),
        "during a take-over, what is copied on the agent's screen must reach the user's \
         clipboard; it has {:?}",
        w.desk.host.paste()
    );

    w.user_gives();
    clip.copy("agent-after");
    thread::sleep(SETTLE);
    assert_ne!(
        w.desk.host.paste().as_deref(),
        Some("agent-after"),
        "after the hand-back, what an app on the agent's screen copied reached the user"
    );
}

#[test]
fn escape_twice_parts_the_clipboards_too() {
    let mut w = World::new();
    let mut agent = w.bare_agent("clip-agent");
    let clip = Clip::start(&w.desk.screen(1));
    w.desk.host.copy("user-secret");
    w.user_clicks(1, 640.0, 400.0);
    assert!(
        wait_until(TIMEOUT, || clip.paste().as_deref() == Some("user-secret")),
        "during a take-over the agent's screen must get the user's clipboard"
    );

    let id = agent.begin("screenshot", json!({}));
    w.user_presses(1, &["Escape", "Escape"]);
    let reply = agent.finish(id, TIMEOUT);
    assert!(!reply.is_error(), "{}", reply.raw);

    assert_ne!(
        clip.paste().as_deref(),
        Some("user-secret"),
        "after Escape twice, an app on the agent's screen still got the user's clipboard"
    );
    w.desk.host.copy("user-secret-2");
    thread::sleep(SETTLE);
    assert_ne!(
        clip.paste().as_deref(),
        Some("user-secret-2"),
        "after Escape twice, what the user copied reached the agent's screen"
    );
}

#[test]
fn a_session_that_ends_during_a_take_over_leaves_the_users_clipboard_alone() {
    let mut w = World::new();
    let mut agent = w.bare_agent("clip-agent");
    let clip = Clip::start(&w.desk.screen(1));
    w.desk.host.copy("user-own");
    w.user_clicks(1, 640.0, 400.0);
    assert!(
        wait_until(TIMEOUT, || clip.paste().as_deref() == Some("user-own")),
        "during a take-over the agent's screen must get the user's clipboard"
    );

    drop(clip);
    agent.kill();
    assert!(
        wait_until(TIMEOUT, || !w.has_view(1)),
        "the view of a session that ended stayed\n{}",
        w.report()
    );
    assert_eq!(
        w.desk.host.paste().as_deref(),
        Some("user-own"),
        "the session's end took the user's clipboard with it"
    );

    // The take-over ended with the session: the next agent's screen has a
    // clipboard of its own.
    let _next = w.bare_agent("next-agent");
    let next = Clip::start(&w.desk.screen(2));
    thread::sleep(SETTLE);
    assert_ne!(
        next.paste().as_deref(),
        Some("user-own"),
        "after a session ended during a take-over, the next agent's screen got the user's \
         clipboard"
    );
}

/// The harness itself: the host keeps what the user copies, and on an
/// agent's screen what one app copies another can paste, as the prototype found.
#[test]
fn the_harness_copies_and_pastes() {
    let mut w = World::new();
    w.desk.host.copy("on-the-desktop");
    assert_eq!(w.desk.host.paste().as_deref(), Some("on-the-desktop"));

    let _agent = w.bare_agent("clip-agent");
    let first = Clip::start(&w.desk.screen(1));
    first.copy("between-apps");
    let second = Clip::start(&w.desk.screen(1));
    assert!(
        wait_until(TIMEOUT, || second.paste().as_deref()
            == Some("between-apps")),
        "an app on the agent's screen can't paste what another copied: {:?}",
        second.paste()
    );
}
