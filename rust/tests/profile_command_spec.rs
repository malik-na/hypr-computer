//! #33, part 1: the user makes, lists and deletes saved browser profiles
//! with `hypr-computer profile new|list|delete <name>`. They live in the
//! user's data folder, open to the user only, and outlast the daemon. Written
//! from the spec alone; the interface is at the top of spec33/mod.rs.
//!
//! No browser is needed here.
#![forbid(unsafe_code)]

mod spec33;

use std::fs;

use spec33::{Daemon, Env, hypr_computer, listed, mode, new_profile, user_runs};

#[test]
fn a_new_profile_is_a_private_folder_in_the_users_data() {
    let env = Env::new();
    let ran = user_runs(&env, &["profile", "new", "work"]);
    assert!(
        ran.said().contains("work"),
        "`hypr-computer profile new work` should name the profile it made: {ran:?}"
    );
    let dir = env.home().join(".local/share/hypr-computer/profiles/work");
    assert!(
        dir.is_dir(),
        "without $XDG_DATA_HOME the profile should be {}",
        dir.display()
    );
    assert_eq!(
        fs::read_dir(&dir).unwrap().count(),
        0,
        "a new profile is empty"
    );
    assert_eq!(
        mode(&dir),
        0o700,
        "{} must be open to the user only",
        dir.display()
    );
    for parent in [
        env.home().join(".local/share/hypr-computer"),
        env.home().join(".local/share/hypr-computer/profiles"),
    ] {
        assert_eq!(
            mode(&parent) & 0o077,
            0,
            "{} must be open to the user only (it is {:o})",
            parent.display(),
            mode(&parent)
        );
    }
    // Never among the user's own browser's folders.
    assert!(!env.home().join(".config/chromium").exists());
    assert!(!env.home().join(".config/google-chrome").exists());
}

#[test]
fn profiles_go_under_xdg_data_home_when_it_is_set() {
    let env = Env::with_data_home();
    new_profile(&env, "work");
    let dir = env.path("data/hypr-computer/profiles/work");
    assert!(dir.is_dir(), "the profile should be {}", dir.display());
    assert_eq!(mode(&dir), 0o700);
    assert!(
        !env.home().join(".local/share/hypr-computer").exists(),
        "with $XDG_DATA_HOME set, nothing goes under ~/.local/share"
    );
    assert_eq!(listed(&env), ["work"]);
}

#[test]
fn list_shows_the_profiles_by_name_and_nothing_else() {
    let env = Env::new();
    let none = user_runs(&env, &["profile", "list"]);
    assert_eq!(
        none.lines(),
        Vec::<String>::new(),
        "with no profile, list prints no name on stdout: {none:?}"
    );

    for name in ["work", "GitHub-2", "a.b_c"] {
        new_profile(&env, name);
    }
    assert_eq!(
        listed(&env),
        ["GitHub-2", "a.b_c", "work"],
        "list prints each name on a line of its own, sorted"
    );
}

#[test]
fn a_name_already_taken_is_refused_and_the_profile_kept() {
    let env = Env::new();
    new_profile(&env, "work");
    // What a browser would have left in it.
    let cookies = env.profile("work").join("Default/Cookies");
    fs::create_dir_all(cookies.parent().unwrap()).unwrap();
    fs::write(&cookies, "the user's logins").unwrap();

    let again = hypr_computer(&env, &["profile", "new", "work"]);
    assert_eq!(
        again.code,
        Some(1),
        "making a profile that exists should fail: {again:?}"
    );
    assert!(
        again.said().contains("work") && again.said().contains("exists"),
        "the error should say profile work exists already: {again:?}"
    );
    assert_eq!(
        fs::read_to_string(&cookies).unwrap(),
        "the user's logins",
        "the existing profile was changed"
    );
    assert_eq!(listed(&env), ["work"]);
}

#[test]
fn delete_removes_a_profile_and_everything_in_it() {
    let env = Env::new();
    new_profile(&env, "work");
    new_profile(&env, "keep");
    let inside = env.profile("work").join("Default/Local Storage/leveldb");
    fs::create_dir_all(&inside).unwrap();
    fs::write(inside.join("000003.log"), "x").unwrap();

    let ran = user_runs(&env, &["profile", "delete", "work"]);
    assert!(
        ran.said().contains("work"),
        "delete should name it: {ran:?}"
    );
    assert!(
        !env.profile("work").exists(),
        "the deleted profile's folder is still there"
    );
    assert!(env.profile("keep").is_dir(), "another profile went too");
    assert_eq!(listed(&env), ["keep"]);

    // A name that was deleted may be made again, empty.
    new_profile(&env, "work");
    assert_eq!(fs::read_dir(env.profile("work")).unwrap().count(), 0);
}

#[test]
fn deleting_a_profile_that_doesnt_exist_fails() {
    let env = Env::new();
    new_profile(&env, "keep");
    let ran = hypr_computer(&env, &["profile", "delete", "nope"]);
    assert_eq!(ran.code, Some(1), "{ran:?}");
    assert!(
        ran.said().contains("nope"),
        "the error should name the profile: {ran:?}"
    );
    assert_eq!(listed(&env), ["keep"]);
}

#[test]
fn a_name_that_isnt_a_plain_folder_name_is_refused_and_nothing_is_made() {
    let env = Env::new();
    new_profile(&env, "keep");
    // A folder a name with a '/' would reach: `delete ../escape` must not.
    let escape = env.home().join(".local/share/hypr-computer/escape");
    fs::create_dir(&escape).unwrap();
    let too_long = "a".repeat(65);
    for bad in [
        "",
        ".",
        "..",
        ".hidden",
        "-x",
        "../escape",
        "a/b",
        "/tmp/abs",
        "a b",
        "naïve",
        too_long.as_str(),
    ] {
        for command in ["new", "delete"] {
            let ran = hypr_computer(&env, &["profile", command, bad]);
            assert_eq!(
                ran.code,
                Some(2),
                "`hypr-computer profile {command} {bad:?}` should be refused as a bad name: {ran:?}"
            );
            assert!(
                ran.said().contains("letters"),
                "the error for {bad:?} should say what a name may hold: {ran:?}"
            );
        }
    }
    // Nothing was made, in the profiles' folder or next to it.
    assert_eq!(listed(&env), ["keep"]);
    let mut there: Vec<String> = fs::read_dir(env.profiles_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with("keep"))
        .collect();
    there.sort();
    assert!(
        there
            .iter()
            .all(|n| n.starts_with('.') || n.ends_with(".lock")),
        "a refused name left {there:?} in {}",
        env.profiles_dir().display()
    );
    assert!(
        escape.is_dir(),
        "`profile delete ../escape` deleted {}",
        escape.display()
    );
    assert!(!env.home().join(".local/share/hypr-computer/a").exists());
    // ... and the one that is there is untouched.
    assert!(env.profile("keep").is_dir());
}

#[test]
fn the_profile_command_is_listed_and_misused_forms_are_usage_errors() {
    let env = Env::new();
    let help = user_runs(&env, &["help"]);
    for words in ["profile new", "profile list", "profile delete"] {
        assert!(
            help.said().contains(words),
            "`hypr-computer help` should list `{words}`: {help:?}"
        );
    }
    for args in [
        &["profile"][..],
        &["profile", "new"],
        &["profile", "delete"],
        &["profile", "frob", "work"],
        &["profile", "new", "work", "extra"],
        &["profile", "list", "extra"],
    ] {
        let ran = hypr_computer(&env, args);
        assert_eq!(ran.code, Some(2), "`hypr-computer {args:?}`: {ran:?}");
    }
    assert!(
        !env.profiles_dir().join("work").exists(),
        "a usage error made a profile"
    );
}

/// The user sets profiles up whether or not an agent has started the
/// daemon, and they outlast it: stopped, killed, started again.
#[test]
fn profiles_outlast_the_daemon() {
    let env = Env::new();
    let browser = env.path("bin/no-browser-needed");
    // Made before any daemon runs.
    new_profile(&env, "before");
    let mut daemon = Daemon::with_browser(&env, &browser);
    // Made, listed and deleted while it runs.
    new_profile(&env, "during");
    new_profile(&env, "gone");
    user_runs(&env, &["profile", "delete", "gone"]);
    assert_eq!(listed(&env), ["before", "during"]);
    let kept = env.profile("during").join("Default/Cookies");
    fs::create_dir_all(kept.parent().unwrap()).unwrap();
    fs::write(&kept, "logins").unwrap();

    daemon.stop();
    assert_eq!(
        listed(&env),
        ["before", "during"],
        "stopping the daemon lost profiles"
    );
    let mut daemon = Daemon::with_browser(&env, &browser);
    assert_eq!(listed(&env), ["before", "during"]);
    daemon.kill_9();
    assert_eq!(
        listed(&env),
        ["before", "during"],
        "killing the daemon lost profiles"
    );
    let _daemon = Daemon::with_browser(&env, &browser);
    assert_eq!(listed(&env), ["before", "during"]);
    assert_eq!(fs::read_to_string(&kept).unwrap(), "logins");
    assert_eq!(mode(&env.profile("during")), 0o700);
}
