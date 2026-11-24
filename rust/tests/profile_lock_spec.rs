//! #33, part 3: two sessions can't use the same profile at once. The second
//! is told plainly that it is in use, and the first goes on unaffected. The
//! profile is free again once the first session has ended, however it
//! ended: its agent left, was killed, or its daemon was. The user can't
//! delete a profile in use. Written from the spec (specs/Profiles.tla); the
//! interface is at the top of spec33/mod.rs.
//!
//! Every test here needs a browser; each passes with a SKIPPED note where no
//! Chromium-family browser is installed (or `HC33_BROWSER=none`).
#![forbid(unsafe_code)]

mod spec33;

use std::thread;

use spec33::{
    BLUE, COOKIE, GREEN, Reply, Shim, TIMEOUT, World, YELLOW, hypr_computer, listed, new_profile,
    wait_until,
};

/// Whether `reply` refuses the profile `name` as in use.
fn in_use(reply: &Reply, name: &str) -> bool {
    let text = reply.text().to_lowercase();
    reply.is_error() && text.contains(name) && text.contains("in use")
}

/// The acceptance criterion: two sessions can't use the same profile at
/// once, with a clear error; and the first is unaffected.
#[test]
fn a_second_session_is_told_the_profile_is_in_use_and_the_first_goes_on() {
    let w = World::start();
    if !w.has_browser() {
        return;
    }
    new_profile(&w.env, "work");
    let mut first = w.agent();
    w.sign_in(&mut first, "work");
    let launches = w.browser.launches().len();

    let mut second = w.agent();
    let refused = second.open_in(&w.web.url("/other"), "work");
    assert!(
        in_use(&refused, "work"),
        "a second session's open_url in profile work should be refused, saying it is in use: {}",
        refused.raw
    );
    assert_eq!(
        w.browser.launches().len(),
        launches,
        "the refused call started a browser"
    );
    assert!(
        w.web.requests_for("/other").is_empty(),
        "the refused page was loaded anyway (in the first session's browser?)"
    );

    // The first session is unaffected: its page is still on its screen, and
    // it goes on in its profile.
    assert!(
        first.screenshot().shows(BLUE),
        "the first session's screen changed"
    );
    first.open_in_ok(&w.web.url("/again"), "work");
    assert!(first.screenshot().shows(GREEN));
    assert!(w.last_cookie("/again").contains(COOKIE));

    // The refused session goes on too; a fresh page has no login.
    second.open(&w.web.url("/other"));
    assert!(second.screenshot().shows(YELLOW));
    assert!(!w.last_cookie("/other").contains("hc33"));
}

/// The refused session may ask again, and gets the profile, logins and all,
/// once the first one has left.
#[test]
fn the_profile_is_free_again_when_its_session_ends() {
    let w = World::start();
    if !w.has_browser() {
        return;
    }
    new_profile(&w.env, "work");
    let mut first = w.agent();
    w.sign_in(&mut first, "work");
    let mut second = w.agent();
    assert!(in_use(
        &second.open_in(&w.web.url("/again"), "work"),
        "work"
    ));

    first.close_stdin();
    let mut reply = None;
    let got = wait_until(TIMEOUT, || {
        let r = second.open_in(&w.web.url("/again"), "work");
        let ok = !r.is_error();
        reply = Some(r);
        ok
    });
    assert!(
        got,
        "the profile wasn't free after its session ended: {:?}",
        reply.map(|r| r.raw)
    );
    assert!(second.screenshot().shows(GREEN));
    assert!(
        w.last_cookie("/again").contains(COOKIE),
        "the next session lost the login: {:?}",
        w.last_cookie("/again")
    );
}

/// An agent killed with kill -9 never says goodbye. Its session still ends,
/// the profile is free again, and what it signed in to is kept: the daemon
/// shuts its browser down, not the agent.
#[test]
fn the_profile_is_free_again_and_kept_when_its_agent_is_killed() {
    let w = World::start();
    if !w.has_browser() {
        return;
    }
    new_profile(&w.env, "work");
    let mut first = w.agent();
    w.sign_in(&mut first, "work");
    first.kill_9();
    w.wait_browsers_gone("the killed agent's session ended");

    let mut next = w.agent();
    next.open_in_ok(&w.web.url("/again"), "work");
    assert!(next.screenshot().shows(GREEN));
    assert!(
        w.last_cookie("/again").contains(COOKIE),
        "the login made before the agent was killed was lost: {:?}",
        w.last_cookie("/again")
    );
}

/// Of two sessions that ask for a profile at the same moment, exactly one
/// gets it.
#[test]
fn of_two_sessions_asking_at_once_exactly_one_gets_the_profile() {
    let w = World::start();
    if !w.has_browser() {
        return;
    }
    new_profile(&w.env, "work");
    let a = w.agent();
    let b = w.agent();
    let ask = |mut shim: Shim, url: String| {
        thread::spawn(move || {
            let reply = shim.open_in(&url, "work");
            (shim, reply)
        })
    };
    let ta = ask(a, w.web.url("/check"));
    let tb = ask(b, w.web.url("/again"));
    let (mut a, ra) = ta.join().unwrap();
    let (mut b, rb) = tb.join().unwrap();

    let won: Vec<bool> = [&ra, &rb].iter().map(|r| !r.is_error()).collect();
    assert_eq!(
        won.iter().filter(|w| **w).count(),
        1,
        "exactly one of two sessions asking at once should get the profile: {} / {}",
        ra.raw,
        rb.raw
    );
    let (winner, loser, colour, lost) = if won[0] {
        (&mut a, &rb, BLUE, "/again")
    } else {
        (&mut b, &ra, GREEN, "/check")
    };
    assert!(
        in_use(loser, "work"),
        "the loser should be told the profile is in use: {}",
        loser.raw
    );
    assert!(
        winner.screenshot().shows(colour),
        "the winner's page isn't shown"
    );
    assert!(
        w.web.requests_for(lost).is_empty(),
        "the loser's page was loaded anyway (in the winner's browser?)"
    );
    assert_eq!(w.browser.launches().len(), 1, "two browsers were started");
}

/// The user can't delete a profile an agent is using; once it is free, they
/// can.
#[test]
fn a_profile_in_use_cant_be_deleted() {
    let w = World::start();
    if !w.has_browser() {
        return;
    }
    new_profile(&w.env, "work");
    let mut a = w.agent();
    w.sign_in(&mut a, "work");

    let ran = hypr_computer(&w.env, &["profile", "delete", "work"]);
    assert_eq!(
        ran.code,
        Some(1),
        "deleting a profile in use should fail: {ran:?}"
    );
    assert!(
        ran.said().contains("work") && ran.said().contains("in use"),
        "the error should say profile work is in use: {ran:?}"
    );
    assert!(w.env.profile("work").is_dir(), "the profile was deleted");
    assert_eq!(listed(&w.env), ["work"]);
    // The agent's browser goes on with its login.
    a.open_in_ok(&w.web.url("/again"), "work");
    assert!(w.last_cookie("/again").contains(COOKIE));

    a.close_stdin();
    w.wait_browsers_gone("the session ended");
    let deleted = wait_until(TIMEOUT, || {
        hypr_computer(&w.env, &["profile", "delete", "work"]).code == Some(0)
    });
    assert!(
        deleted,
        "the profile couldn't be deleted after its session ended"
    );
    assert!(!w.env.profile("work").exists());
    assert_eq!(listed(&w.env), Vec::<String>::new());
}

/// A daemon killed with kill -9 holds nothing afterwards: the profile its
/// session had is free for the next daemon's sessions once that session's
/// browser is gone, and never used by two browsers meanwhile.
#[test]
fn the_profile_is_free_again_after_its_daemon_is_killed() {
    let mut w = World::start();
    if !w.has_browser() {
        return;
    }
    new_profile(&w.env, "work");
    let mut first = w.agent();
    w.sign_in(&mut first, "work");
    w.daemon().kill_9();
    drop(first);
    w.restart_daemon();

    let mut next = w.agent();
    let mut last = None;
    let got = wait_until(TIMEOUT, || {
        let r = next.open_in(&w.web.url("/again"), "work");
        let ok = !r.is_error();
        if !ok {
            assert!(
                in_use(&r, "work"),
                "while the old browser runs, the profile is in use; open_url said: {}",
                r.raw
            );
        }
        last = Some(r);
        ok
    });
    assert!(
        got,
        "after the daemon was killed the profile never became free: {:?}",
        last.map(|r| r.raw)
    );
    assert!(
        next.screenshot().shows(GREEN),
        "the page didn't open on the new session's screen"
    );
    // Exactly one browser runs now: the new session's.
    let launches = w.browser.launches();
    let running: Vec<u32> = launches
        .iter()
        .copied()
        .filter(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists())
        .collect();
    assert!(
        running.len() <= 1,
        "two browsers run on the profile: {running:?}"
    );
}
