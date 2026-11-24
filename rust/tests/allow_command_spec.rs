//! #31: the `hypr-computer allow` command itself: what it says, and what it
//! does with arguments it doesn't take. Written from the spec alone;
//! spec31/ says what interface it assumes.
#![forbid(unsafe_code)]

mod spec31;

use spec31::{BLUE, GREEN, ORANGE, PURPLE, World, assert_refused};

#[test]
fn allow_says_what_agents_may_do_and_list_shows_it() {
    let world = World::new();
    let listed = world.user_runs(&["allow"]);
    assert!(listed.stdout.contains("No limits"), "{listed:?}");

    let ran = world.user_runs(&["allow", "notes,paint", "browser"]);
    assert!(
        ran.stdout.contains("notes") && ran.stdout.contains("paint"),
        "`allow` should say which apps agents may use: {ran:?}"
    );
    for args in [&["allow"][..], &["allow", "--list"]] {
        let listed = world.user_runs(args);
        for app in ["notes", "paint", "browser"] {
            assert!(
                listed.stdout.contains(app),
                "`hypr-computer {}` should list {app}: {listed:?}",
                args.join(" ")
            );
        }
        assert!(!listed.stdout.contains("No limits"), "{listed:?}");
        assert!(
            !listed.stdout.to_lowercase().contains("until"),
            "a limit without --for has no end: {listed:?}"
        );
    }

    world.user_runs(&["allow", "--read-only", "--for", "1h"]);
    let listed = world.user_runs(&["allow", "--list"]);
    assert!(
        listed.stdout.to_lowercase().contains("read-only")
            && listed.stdout.to_lowercase().contains("until"),
        "{listed:?}"
    );
    assert!(
        !listed.stdout.contains("notes"),
        "a new `allow` replaces the old limits: {listed:?}"
    );

    let off = world.user_runs(&["allow", "--off"]);
    assert!(
        !off.stdout.trim().is_empty(),
        "`allow --off` should say so: {off:?}"
    );
    let listed = world.user_runs(&["allow", "--list"]);
    assert!(listed.stdout.contains("No limits"), "{listed:?}");
}

/// Arguments `allow` doesn't take exit 2, say what is wrong and how to use
/// the command, and change nothing.
#[test]
fn a_bad_allow_command_changes_nothing_and_exits_2() {
    let mut world = World::new();
    world.user_runs(&["allow", "notes"]);

    for (args, names) in [
        (&["allow", "--for", "30m"][..], ""),
        (&["allow", "paint", "--for"], "--for"),
        (&["allow", "paint", "--for", "soon"], "soon"),
        (&["allow", "paint", "--for", "0m"], "0m"),
        (&["allow", "--sites"], "--sites"),
        (&["allow", "--sites", "*.example.com"], "*.example.com"),
        (&["allow", "--bogus"], "--bogus"),
        (&["allow", "/usr/bin/paint"], "/usr/bin/paint"),
        (&["allow", "--read-only", "paint"], "--read-only"),
        (&["allow", "--off", "paint"], ""),
        (&["allow", "--list", "paint"], ""),
    ] {
        let ran = world.hypr_computer(args);
        assert_eq!(
            ran.code,
            Some(2),
            "`hypr-computer {}` must exit 2: {ran:?}",
            args.join(" ")
        );
        assert!(
            ran.stderr.contains(names) && ran.stderr.to_lowercase().contains("usage"),
            "`hypr-computer {}` should name {names:?} and show the usage on stderr: {ran:?}",
            args.join(" ")
        );
    }

    // The limit from before still holds, unchanged.
    let listed = world.user_runs(&["allow", "--list"]);
    assert!(
        listed.stdout.contains("notes") && !listed.stdout.contains("paint"),
        "a refused command changed the limits: {listed:?}"
    );
    assert!(
        !listed.stdout.to_lowercase().contains("read-only"),
        "{listed:?}"
    );
    let mut agent = world.agent("alpha");
    let (reply, _) = agent.try_launch(&world.env, "paint", (ORANGE, PURPLE), &[]);
    assert_refused(&reply, "paint", "launch_app of paint");
    let notes = agent.launch(&world.env, "notes", (GREEN, BLUE));
    agent.click_reaches(&notes);
}

#[test]
fn the_usage_mentions_allow() {
    let world = World::new();
    for args in [&["help"][..], &["--help"]] {
        let ran = world.user_runs(args);
        assert!(
            ran.stdout.contains("allow") && ran.stdout.contains("--for"),
            "`hypr-computer {}` should show the allow command: {ran:?}",
            args.join(" ")
        );
    }
}
