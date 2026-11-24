//! #31: password managers and password prompts are never launched or shown
//! on an agent's screen, whatever the limits. Written from the spec (#31,
//! specs/Limits.tla) alone; spec31/ says what interface it assumes.
//!
//! Every password manager here is a copy of hc-test-app painted all
//! `SECRET`, a colour no other app in these tests uses, so a screenshot
//! that shows any of it has shown a password manager.
#![forbid(unsafe_code)]

mod spec31;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use spec31::{
    BLUE, GREEN, ORANGE, PURPLE, QUIET, SECRET, TIMEOUT, World, assert_refused, hex, quote,
    running, wait_until,
};

const NOTES: (spec31::Rgb, spec31::Rgb) = (GREEN, BLUE);
const VAULT: (spec31::Rgb, spec31::Rgb) = (SECRET, SECRET);

/// The processes running `program` now.
fn pids_of(program: &Path) -> Vec<u32> {
    let Ok(dir) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_string_lossy().parse::<u32>().ok())
        .filter(|pid| {
            fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe| exe == program)
                && running(*pid)
        })
        .collect()
}

/// Waits until no process runs `program`; whether none does.
fn all_ended(program: &Path) -> bool {
    wait_until(TIMEOUT, || pids_of(program).is_empty())
}

/// hc-test-app, started straight on the screen whose Wayland socket is
/// `socket`, as `program`, painted all `SECRET`: as an app on the screen
/// might start a prompt there, not through launch_app.
fn start_on_screen(socket: &Path, program: &Path, extra: &[&str], log: &Path) -> Child {
    std::process::Command::new(program)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("WAYLAND_DISPLAY", socket)
        .args(["--top", &hex(SECRET), "--bottom", &hex(SECRET), "--log"])
        .arg(log)
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

struct Reaped(Vec<Child>);

impl Drop for Reaped {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn a_password_manager_is_never_launched_limits_or_not() {
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    let names = [
        "keepassxc",
        "KeePassXC",
        "1password",
        "bitwarden",
        "pinentry-gnome3",
        "hyprpolkitagent",
    ];
    for name in names {
        let (reply, app) = agent.try_launch(&world.env, name, VAULT, &[]);
        assert_refused(&reply, "password", &format!("launch_app of {name}"));
        thread::sleep(Duration::from_millis(300));
        assert!(!app.log.exists(), "{name} was launched");
    }

    // Naming one in a limit doesn't let it in either.
    world.user_runs(&["allow", "keepassxc,notes"]);
    let (reply, app) = agent.try_launch(&world.env, "keepassxc", VAULT, &[]);
    assert_refused(
        &reply,
        "password",
        "launch_app of keepassxc under `allow keepassxc`",
    );
    let notes = agent.launch(&world.env, "notes", NOTES);
    agent.click_reaches(&notes);
    thread::sleep(QUIET);
    assert!(!app.log.exists(), "keepassxc was launched");
    assert!(!agent.shim.screenshot().has(SECRET));
}

/// A program with an innocent name whose window says it is a password
/// manager (by its Wayland app id) is never shown, and it ends.
#[test]
fn a_window_with_a_password_managers_app_id_is_never_shown() {
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    let notes = agent.launch(&world.env, "notes", NOTES);

    for app_id in ["org.keepassxc.KeePassXC", "1Password", "gcr-prompter"] {
        let program = world.env.app_program("helper");
        let (reply, app) =
            agent.launch_program(&world.env, &program, "helper", VAULT, &["--app-id", app_id]);
        assert_refused(
            &reply,
            "password",
            &format!("launch_app of a program whose window has the app id {app_id}"),
        );
        let end = Instant::now() + QUIET;
        while Instant::now() < end {
            let shot = agent.shim.screenshot();
            assert!(
                !shot.has(SECRET),
                "a window with the app id {app_id} was shown: {shot:?}"
            );
            assert!(
                shot.shows(NOTES),
                "notes should still be in front: {shot:?}"
            );
        }
        assert!(
            all_ended(&program),
            "the program behind {app_id}'s window still runs: {:?}",
            pids_of(&program)
        );
        assert_eq!(
            app.log
                .events()
                .into_iter()
                .filter(|e| e["event"] == "ready")
                .count(),
            0,
            "{app_id}'s window was mapped"
        );
    }
    agent.click_reaches(&notes);
}

/// An app the agent may use starts a password prompt itself (as gpg starts
/// pinentry): the prompt is never shown, gets no input, and ends; the app
/// that started it stays.
#[test]
fn a_password_prompt_an_app_starts_is_never_shown() {
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    let env = &world.env;
    let prompt = env.app_program("pinentry-gnome3");
    let editor_bin = env.app_program("editor-bin");
    let prompt_log = env.path("prompt.log");
    let started = env.path("prompt.started");
    let editor = env.script(
        "editor",
        &format!(
            "( sleep 2; touch {started}; exec {prompt} --top {s} --bottom {s} --log {log} ) &\n\
             exec {editor_bin} \"$@\"",
            started = quote(&started.to_string_lossy()),
            prompt = quote(&prompt.to_string_lossy()),
            s = hex(SECRET),
            log = quote(&prompt_log.to_string_lossy()),
            editor_bin = quote(&editor_bin.to_string_lossy()),
        ),
    );
    let (reply, app) = agent.launch_program(env, &editor, "editor", NOTES, &[]);
    assert!(
        !reply.is_error(),
        "launch_app of editor failed: {}",
        reply.raw
    );
    assert!(
        app.log
            .wait_for(|e| e.iter().any(|e| e["event"] == "ready"))
    );
    assert!(
        wait_until(TIMEOUT, || started.exists()),
        "the editor never started its prompt"
    );

    let end = Instant::now() + Duration::from_secs(4);
    while Instant::now() < end {
        let shot = agent.shim.screenshot();
        assert!(!shot.has(SECRET), "the password prompt was shown: {shot:?}");
        let _ = agent.shim.call("click", json!({"x": 640, "y": 400}));
    }
    assert!(
        all_ended(&prompt),
        "the password prompt still runs: {:?}",
        pids_of(&prompt)
    );
    let got: Vec<Value> = spec31::Log {
        path: prompt_log.clone(),
    }
    .inputs();
    assert!(got.is_empty(), "the password prompt got input: {got:?}");
    assert!(
        !pids_of(&editor_bin).is_empty(),
        "the app that started the prompt was ended too"
    );
    agent.click_reaches(&app);
}

/// The v0.8 bug (fde9289, fixed in e6d3101): a password manager was kept out
/// of a screenshot only if it was there when the capture began, so one that
/// appeared during the capture was in the picture. Here password managers
/// keep appearing, started straight on the screen, while the agent takes
/// screenshot after screenshot and clicks: none is ever in a picture, none
/// gets input, and each one ends.
#[test]
fn a_password_manager_appearing_during_screenshots_is_never_in_one() {
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    let notes = agent.launch(&world.env, "notes", NOTES);
    let socket = world.env.screen_socket(agent.session);
    let vault = world.env.app_program("keepassxc");
    let innocent = world.env.app_program("helper");
    let logs: Vec<PathBuf> = (0..10)
        .map(|i| world.env.path(&format!("vault-{i}.log")))
        .collect();
    let starter = {
        let (vault, innocent, logs) = (vault.clone(), innocent.clone(), logs.clone());
        thread::spawn(move || {
            let mut started = Vec::new();
            for (i, log) in logs.iter().enumerate() {
                // Every other one is known by its app id rather than its program.
                let child = if i % 2 == 0 {
                    start_on_screen(&socket, &vault, &[], log)
                } else {
                    let id = ["--app-id", "org.keepassxc.KeePassXC"];
                    start_on_screen(&socket, &innocent, &id, log)
                };
                started.push(child);
                thread::sleep(Duration::from_millis(250));
            }
            started
        })
    };

    let mut shots = 0;
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end || !starter.is_finished() {
        let shot = agent.shim.screenshot();
        shots += 1;
        assert!(
            !shot.has(SECRET),
            "screenshot {shots} showed a password manager that appeared while it was taken: \
             {shot:?}"
        );
        if shots % 3 == 0 {
            let _ = agent.shim.call("click", json!({"x": 640, "y": 400}));
        }
    }
    let children = Reaped(starter.join().unwrap());
    assert!(shots >= 10, "only {shots} screenshots were taken");
    assert!(
        all_ended(&vault) && all_ended(&innocent),
        "password managers still run: {:?} {:?}",
        pids_of(&vault),
        pids_of(&innocent)
    );
    for log in &logs {
        let got = spec31::Log { path: log.clone() }.inputs();
        assert!(got.is_empty(), "a password manager got input: {got:?}");
    }
    agent.click_reaches(&notes);
    drop(children);
    // The other apps work as before.
    let other = agent.launch(&world.env, "paint", (ORANGE, PURPLE));
    agent.click_reaches(&other);
}
