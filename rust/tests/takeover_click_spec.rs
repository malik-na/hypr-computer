//! #30: clicking into an agent's live view takes over its screen. The user's
//! pointer and keys then reach it, its view shows a banner, and the agent's
//! calls wait until `hypr-computer give` hands it back; then each runs once.
//! The interface these tests assume is at the top of tests/spec30/mod.rs.
#![forbid(unsafe_code)]

mod spec30;
mod support;
mod view_support;

use serde_json::json;

use spec30::probe::{presses, typed};
use spec30::{BANNER_PIXELS, TIMEOUT, WAITS, World, buttons, inputs, one_press_at, pixels_unlike};
use view_support::host::BTN_LEFT;

#[test]
fn clicking_into_a_view_takes_over_and_the_click_itself_reaches_nothing() {
    let mut w = World::new();
    let agent = w.agent("spec30-agent");
    let probe = agent.probe();

    let mark = probe.mark();
    w.user_hovers(1, 500.0, 300.0);
    let got = probe.settle(mark);
    assert!(
        inputs(&got).is_empty(),
        "moving over a watch-only view reached the agent's app: {got:?}"
    );

    let mark = probe.mark();
    w.user_clicks(1, 300.0, 200.0);
    let got = probe.settle(mark);
    assert!(
        buttons(&got).is_empty(),
        "the click that takes over reached the agent's app: {got:?}"
    );

    w.wait_taken_over(&agent, 300.0, 200.0);
    let mark = probe.mark();
    w.user_clicks(1, 900.0, 600.0);
    let ok = probe.wait_for(mark, |e| buttons(e).iter().any(|b| !b.1));
    assert!(
        ok && one_press_at(&probe.presses_since(mark), 900.0, 600.0),
        "once the user has taken over, their click must reach the agent's app where they \
         clicked, (900, 600): {:?}\n{}",
        probe.since(mark),
        w.report()
    );
    w.user_gives();
}

#[test]
fn once_taken_over_the_users_keys_reach_the_agents_app() {
    let mut w = World::new();
    let agent = w.agent("spec30-agent");
    let probe = agent.probe();
    w.take_over(&agent, 640.0, 400.0);

    let mark = probe.mark();
    w.user_presses(1, &["h", "i"]);
    assert_eq!(
        probe.wait_typed(mark, "hi"),
        "hi",
        "the user's keys must reach the screen they took over: {:?}\n{}",
        probe.since(mark),
        w.report()
    );
    w.user_gives();
}

#[test]
fn the_agents_click_waits_during_a_take_over_and_lands_once_after_give() {
    let mut w = World::new();
    let mut agent = w.agent("waiting-agent");
    w.take_over(&agent, 640.0, 400.0);

    let mark = agent.probe().mark();
    let id = agent.begin("click", json!({"x": 100, "y": 100}));
    agent.assert_waits(id, WAITS, "a click");
    assert!(
        agent.probe().presses_since(mark).is_empty(),
        "the agent's click landed while the user had its screen: {:?}",
        agent.probe().since(mark)
    );

    let said = w.user_gives();
    assert!(
        said.contains("waiting-agent"),
        "`hypr-computer give` must name the agent it handed back: {said:?}"
    );
    let reply = agent.finish(id, TIMEOUT);
    assert!(
        !reply.is_error(),
        "the waiting click must run after the hand-back: {}",
        reply.raw
    );
    assert!(
        reply.says_taken_over(),
        "the agent must be told the user took over its screen: {}",
        reply.raw
    );
    agent.probe().wait_for(mark, |e| !presses(e).is_empty());
    let got = agent.probe().settle(mark);
    assert!(
        one_press_at(&presses(&got), 100.0, 100.0),
        "the waiting click must land once, at (100, 100): {got:?}"
    );

    // The agent has its screen back: its next call is served at once.
    let mark = agent.probe().mark();
    agent.ok("click", json!({"x": 200, "y": 300}));
    agent.probe().wait_for(mark, |e| !presses(e).is_empty());
    assert!(
        one_press_at(&agent.probe().presses_since(mark), 200.0, 300.0),
        "after the hand-back the agent's click must land: {:?}",
        agent.probe().since(mark)
    );
}

#[test]
fn the_agents_screenshot_waits_during_a_take_over_too() {
    let mut w = World::new();
    let mut agent = w.agent("spec30-agent");
    w.take_over(&agent, 640.0, 400.0);

    let id = agent.begin("screenshot", json!({}));
    agent.assert_waits(id, WAITS, "a screenshot");
    w.user_gives();
    let reply = agent.finish(id, TIMEOUT);
    assert!(
        !reply.is_error(),
        "the waiting screenshot must be taken after the hand-back: {}",
        reply.raw
    );
    assert!(
        reply.says_taken_over(),
        "the agent must be told the user took over its screen: {}",
        reply.raw
    );
    let shot = reply.image();
    assert_eq!((shot.width, shot.height), (1280, 800));
}

#[test]
fn the_view_shows_a_banner_only_while_taken_over_and_screenshots_never_do() {
    let mut w = World::new();
    // No app: nothing on the screen changes when the user's pointer comes.
    let mut agent = w.bare_agent("banner-agent");
    let before = agent.screenshot();
    let unlike = || pixels_unlike(&w.view_picture(1), &before);
    assert!(
        spec30::wait_until(TIMEOUT, || unlike() < BANNER_PIXELS),
        "before the take-over the view must show the screen as it is: {} pixels differ",
        unlike()
    );

    w.user_clicks(1, 640.0, 400.0);
    assert!(
        spec30::wait_until(TIMEOUT, || unlike() >= BANNER_PIXELS),
        "while the user has the screen its view must show a banner; only {} pixels differ \
         from the screen\n{}",
        unlike(),
        w.report()
    );
    let id = agent.begin("screenshot", json!({}));
    agent.assert_waits(id, WAITS, "a screenshot");

    w.user_gives();
    let during = agent.finish(id, TIMEOUT);
    assert!(!during.is_error(), "{}", during.raw);
    assert!(
        during.image().pixels_unlike(&before) < BANNER_PIXELS,
        "the banner reached the agent's screenshot"
    );
    assert!(
        spec30::wait_until(TIMEOUT, || unlike() < BANNER_PIXELS),
        "after the hand-back the banner must go: {} pixels differ from the screen",
        unlike()
    );
    assert!(
        agent.screenshot().pixels_unlike(&before) < BANNER_PIXELS,
        "the banner reached the agent's screen"
    );
}

#[test]
fn give_with_no_screen_taken_over_fails_and_changes_nothing() {
    let mut w = World::new();
    let mut agent = w.agent("spec30-agent");

    for when in ["before any take-over", "after a hand-back"] {
        let out = w.hypr_computer(&["give"]);
        assert_eq!(
            out.status.code(),
            Some(1),
            "`hypr-computer give` {when}, with nothing taken over, must exit 1: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !String::from_utf8_lossy(&out.stderr).trim().is_empty(),
            "`hypr-computer give` must say why it failed"
        );
        let mark = agent.probe().mark();
        agent.ok("click", json!({"x": 300, "y": 300}));
        agent.probe().wait_for(mark, |e| !presses(e).is_empty());
        assert!(
            one_press_at(&agent.probe().presses_since(mark), 300.0, 300.0),
            "{when}, the agent's click must land: {:?}",
            agent.probe().since(mark)
        );
        w.take_over(&agent, 640.0, 400.0);
        w.user_gives();
    }
}

/// TakeOverMidCall, black-box: the user clicks in while the agent is typing.
/// The user's keys must come after all of the agent's text, never inside it.
#[test]
fn the_users_keys_never_land_inside_the_agents_text() {
    let mut w = World::new();
    let mut agent = w.agent("typing-agent");
    let text = "abcdefghij".repeat(60);

    let mark = agent.probe().mark();
    let id = agent.begin("type", json!({"text": text}));
    // The call is giving its input when the user clicks in.
    agent.probe().wait_for(mark, |e| !typed(e).is_empty());
    w.user_clicks(1, 640.0, 400.0);
    w.user_presses(1, &["z", "z", "z"]);
    let reply = agent.finish(id, TIMEOUT);
    assert!(
        !reply.is_error(),
        "a call let through before the take-over must end as usual: {}",
        reply.raw
    );
    // Once the call is over, the user has the screen.
    w.user_presses(1, &["z"]);
    let ok = agent.probe().wait_for(mark, |e| typed(e).ends_with('z'));
    let got = typed(&agent.probe().settle(mark));
    assert!(
        ok,
        "once the agent's call ended, the user's keys must reach the screen: {got:?}\n{}",
        w.report()
    );
    assert_eq!(
        got.trim_end_matches('z'),
        text,
        "the user's keys landed inside the agent's text"
    );
    w.user_gives();
}

/// The harness itself: on the host, a click gives a window the keyboard, as
/// Hyprland's click to focus does, and the user's keys then reach it.
#[test]
fn the_hosts_click_gives_a_window_the_keyboard_and_keys_reach_it() {
    let desk = view_support::Desktop::new();
    let colours = [
        [0x80, 0x20, 0x20],
        [0x20, 0x80, 0x20],
        [0x20, 0x20, 0x80],
        [0x80, 0x80, 0x20],
    ];
    let app = view_support::probe::Probe::paint(desk.host.socket(), colours, "plain.app");
    let mut address = None;
    spec30::wait_until(TIMEOUT, || {
        address = desk
            .host
            .windows()
            .into_iter()
            .find(|w| w.app_id == "plain.app")
            .and_then(|w| w.address);
        address.is_some()
    });
    let address = address.expect("the app's window never mapped");
    assert!(
        desk.host.keys(&address, &["a"]).is_err(),
        "keys reached a window the user never clicked"
    );

    desk.host.click(&address, (200.0, 100.0), BTN_LEFT).unwrap();
    desk.host.keys(&address, &["a", "ctrl+v"]).unwrap();
    let got = |what: &str| app.events().iter().filter(|e| e.starts_with(what)).count();
    assert!(
        spec30::wait_until(TIMEOUT, || got("pointer Button") >= 2 && got("key ") >= 6),
        "the click and keys didn't reach the window: {:?}",
        app.events()
    );
    assert_eq!(
        desk.hypr.world().focused.as_deref(),
        Some(address.as_str()),
        "the click didn't focus the window"
    );
}
