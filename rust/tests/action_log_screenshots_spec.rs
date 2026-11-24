//! #34: with `screenshots = true` in the settings, the images agents get are
//! saved with their log, privately, and go with it. Written from the spec
//! alone; spec34/ says what interface it assumes.
#![forbid(unsafe_code)]

mod spec34;

use std::fs;
use std::path::PathBuf;

use serde_json::json;
use spec34::{
    DAY, Record, TIMEOUT, World, assert_stopped, backdate, image_file, is_png, lines_with,
    loose_entries, pictures_under, wait_until,
};

const ON: &str = "[log]\nscreenshots = true\n";

/// The file a record says its screenshot was saved to; it must say one.
#[track_caller]
fn saved_picture(world: &World, record: &Record) -> PathBuf {
    let name = record.value["screenshot"].as_str().unwrap_or_else(|| {
        panic!(
            "with screenshots on, a screenshot's record names its picture: {:?}",
            record.value
        )
    });
    let path = world.log_dir().join(name);
    assert!(
        path.starts_with(world.log_dir()) && !name.contains(".."),
        "{name:?} isn't in the log dir"
    );
    path
}

#[test]
fn a_screenshot_is_saved_as_the_agent_got_it_when_turned_on() {
    let mut world = World::with_config(ON);
    let mut a = world.agent("alpha");
    let start = world.after_setup(1);

    a.shim.ok("click", json!({"x": 300, "y": 200}));
    let image = a.shim.screenshot();

    let all = world.wait_records(1, start + 2);
    let (click, shot) = (&all[start], &all[start + 1]);
    assert_eq!((click.tool(), shot.tool()), ("click", "screenshot"));
    assert!(
        click.value.get("screenshot").is_none(),
        "a click gives the agent no picture, so none is saved: {:?}",
        click.value
    );
    let path = saved_picture(&world, shot);
    let bytes = fs::read(&path).unwrap_or_default();
    assert!(is_png(&bytes), "{} must be a PNG", path.display());
    assert_eq!(
        image_file(&path),
        image,
        "the saved picture must be the one the agent got"
    );
    let loose = loose_entries(&world.state_dir());
    assert!(
        loose.is_empty(),
        "saved pictures are the user's only: {loose:#?}"
    );

    let shown = world.user_runs(&["log", "alpha"]).stdout;
    let shown_path = path.display().to_string();
    assert!(
        lines_with(&shown, "screenshot")
            .iter()
            .any(|l| l.contains(&shown_path)),
        "`hypr-computer log` shows where the screenshot was saved, {shown_path}:\n{shown}"
    );
}

#[test]
fn each_screenshot_is_saved_on_its_own() {
    let mut world = World::with_config(ON);
    let mut a = world.agent("alpha");
    let start = world.after_setup(1);

    let first = a.shim.screenshot();
    a.shim.ok("click", json!({"x": 900, "y": 600}));
    let second = a.shim.screenshot();
    assert_ne!(first, second, "the click must have changed the screen");

    let all = world.wait_records(1, start + 3);
    let (p1, p2) = (
        saved_picture(&world, &all[start]),
        saved_picture(&world, &all[start + 2]),
    );
    assert_ne!(p1, p2, "each screenshot gets a file of its own");
    assert_eq!(image_file(&p1), first);
    assert_eq!(image_file(&p2), second);
}

#[test]
fn a_refused_screenshot_saves_nothing() {
    let mut world = World::with_config(ON);
    let mut a = world.agent("alpha");
    let start = world.after_setup(1);
    let before = pictures_under(&world.log_dir());

    world.user_runs(&["stop", "alpha"]);
    let reply = a.shim.call("screenshot", json!({}));
    assert_stopped(&reply, "the stopped agent's screenshot");

    let all = world.wait_records(1, start + 1);
    assert_eq!(all[start].result(), "stopped", "{:?}", all[start].value);
    assert!(
        all[start].value.get("screenshot").is_none(),
        "a refused screenshot has no picture to save: {:?}",
        all[start].value
    );
    assert_eq!(pictures_under(&world.log_dir()), before);
}

#[test]
fn screenshots_set_off_are_not_saved() {
    let mut world = World::with_config("[log]\nscreenshots = false\n");
    let mut a = world.agent("alpha");
    let start = world.after_setup(1);
    a.shim.screenshot();
    a.shim.ok("click", json!({"x": 900, "y": 600}));
    a.shim.screenshot();

    let all = world.wait_records(1, start + 3);
    assert!(
        all.iter().all(|r| r.value.get("screenshot").is_none()),
        "{all:#?}"
    );
    let pictures = pictures_under(&world.state_dir());
    assert!(pictures.is_empty(), "screenshots are off: {pictures:?}");
}

#[test]
fn saved_screenshots_are_removed_with_their_log() {
    let mut world = World::with_config(ON);
    {
        let mut a = world.agent("alpha");
        a.shim.screenshot();
        a.shim.screenshot();
        world.wait_records(1, 3);
        a.shim.close_stdin();
        assert!(a.shim.wait_exit(), "the shim didn't exit");
    }
    let file = world.session_file(1);
    let pictures = pictures_under(&world.log_dir());
    assert!(
        pictures.len() >= 3,
        "with screenshots on, each of alpha's screenshots is saved: {pictures:?}"
    );
    backdate(&world.log_dir(), 4 * DAY);

    let (_beta, _) = world.connect("beta");

    assert!(
        wait_until(TIMEOUT, || !file.exists()),
        "the 4-day-old log must be removed"
    );
    let left = pictures_under(&world.log_dir());
    assert!(
        wait_until(TIMEOUT, || pictures_under(&world.log_dir()).is_empty()),
        "its screenshots must go with it: {left:?}"
    );
}
