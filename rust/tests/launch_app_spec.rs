//! #25: `launch_app` starts an installed app on the agent's screen, and only
//! there; the app, and everything it starts, ends with the session. Real apps
//! (foot) run on the screen. Written from the spec alone.
//!
//! The tests that need foot pass with a SKIPPED note where it isn't
//! installed.
#![forbid(unsafe_code)]

mod spec25;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;
use spec25::probe::Probe;
use spec25::{
    Daemon, Env, Reap, Shim, TIMEOUT, cmdline, installed, mentions, read_pids, running, wait_until,
};

fn foot_or_skip() -> bool {
    let ok = installed("foot");
    if !ok {
        eprintln!("SKIPPED: foot isn't installed on this machine");
    }
    ok
}

/// A wrapper that notes its pid, arguments and environment, then becomes
/// foot running `script` in sh (`exec` keeps the pid).
fn foot_wrapper(env: &Env, script: &str) -> PathBuf {
    env.script(
        "app.sh",
        &format!(
            "echo $$ >> '{pids}'\nprintf '%s\\n' \"$@\" > '{args}'\nenv > '{envf}'\nexec foot sh -c '{script}'",
            pids = env.path("pids").display(),
            args = env.path("args").display(),
            envf = env.path("appenv").display(),
        ),
    )
}

/// The share of 64 points spread over the screen that differ between two
/// screenshots.
fn changed(a: &spec25::Image, b: &spec25::Image) -> f64 {
    let mut n = 0;
    for i in 0..8 {
        for j in 0..8 {
            let (x, y) = (80 + i * 160, 50 + j * 100);
            if a.pixel(x, y) != b.pixel(x, y) {
                n += 1;
            }
        }
    }
    n as f64 / 64.0
}

#[test]
fn foot_starts_on_the_agents_screen_and_stays_up() {
    if !foot_or_skip() {
        return;
    }
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::ready(&env);
    let blank = shim.screenshot();
    let app = foot_wrapper(&env, "sleep 1000");

    shim.ok("launch_app", json!({"app": app}));
    // "Returns once its window is there": the very next screenshot has it.
    let shot = shim.screenshot();
    assert!(
        changed(&blank, &shot) > 0.9,
        "the screen still looks blank after launch_app"
    );

    let pid = read_pids(&env.path("pids"), 1)[0];
    let _reap = Reap(vec![pid]);
    std::thread::sleep(Duration::from_secs(3));
    assert!(running(pid), "foot exited; it should stay up");
    assert!(cmdline(pid).contains("foot"), "{}", cmdline(pid));
    let later = shim.screenshot();
    assert!(changed(&blank, &later) > 0.9, "foot's window went away");
}

#[test]
fn an_app_named_on_path_starts_with_the_arguments_given() {
    if !foot_or_skip() {
        return;
    }
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::ready(&env);
    let blank = shim.screenshot();
    let out = env.path("args.txt");
    shim.ok(
        "launch_app",
        json!({"app": "foot", "args": ["sh", "-c", format!("printf '%s|' \"$@\" > '{}'; sleep 1000", out.display()), "x", "one two", "thrée"]}),
    );
    assert!(
        changed(&blank, &shim.screenshot()) > 0.9,
        "no window after launch_app foot"
    );
    let mut got = String::new();
    wait_until(TIMEOUT, || {
        got = fs::read_to_string(&out).unwrap_or_default();
        !got.is_empty()
    });
    assert_eq!(
        got, "one two|thrée|",
        "each argument passed as one, unchanged"
    );
}

/// The launched app is put in front of the app already on the screen.
#[test]
fn a_launched_app_fills_the_screen_in_front_of_the_one_there() {
    if !foot_or_skip() {
        return;
    }
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::ready(&env);
    let _probe = Probe::start(&env.screen_socket(1));
    let before = shim.screenshot();
    shim.ok(
        "launch_app",
        json!({"app": "foot", "args": ["sh", "-c", "sleep 1000"]}),
    );
    let after = shim.screenshot();
    assert!(
        changed(&before, &after) > 0.9,
        "the app already there is still in front"
    );
    assert_eq!((after.width, after.height), (1280, 800));
}

/// The app's environment points at the agent's screen, never the user's
/// desktop, even when the daemon was started from inside that desktop.
#[test]
fn a_launched_app_is_on_the_agents_screen_and_not_the_users() {
    if !foot_or_skip() {
        return;
    }
    let env = Env::new();
    let mut cmd = env.command(&spec25::bin("hypr-computer-daemon"));
    // What a daemon started from the user's desktop would inherit.
    cmd.env("WAYLAND_DISPLAY", "wayland-user-desktop")
        .env("DISPLAY", ":77");
    let _daemon = Daemon::spawn(&env, cmd);
    let mut shim = Shim::ready(&env);
    shim.ok(
        "launch_app",
        json!({"app": foot_wrapper(&env, "sleep 1000")}),
    );
    read_pids(&env.path("pids"), 1);

    let mut vars = HashMap::new();
    wait_until(TIMEOUT, || {
        let text = fs::read_to_string(env.path("appenv")).unwrap_or_default();
        vars = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect::<HashMap<_, _>>();
        vars.contains_key("PATH")
    });
    let display = vars.get("WAYLAND_DISPLAY").cloned().unwrap_or_default();
    let socket = if Path::new(&display).is_absolute() {
        PathBuf::from(&display)
    } else {
        PathBuf::from(vars.get("XDG_RUNTIME_DIR").cloned().unwrap_or_default()).join(&display)
    };
    assert_eq!(
        fs::canonicalize(&socket).ok(),
        fs::canonicalize(env.screen_socket(1)).ok(),
        "the app's WAYLAND_DISPLAY is {display:?}, not the agent's screen"
    );
    assert_ne!(
        vars.get("DISPLAY").map(String::as_str),
        Some(":77"),
        "the app could open X11 windows on the user's desktop"
    );
    assert!(!vars.contains_key("WAYLAND_SOCKET"), "{vars:?}");
}

/// Writes the pids of foot, the shell in it, a child of that shell and a
/// child that left the session with setsid, and returns them once listed.
fn foot_with_children(env: &Env, shim: &mut Shim) -> Vec<u32> {
    let pids = env.path("pids");
    let script = format!(
        "echo $$ >> {p}; sleep 1000 & echo $! >> {p}; setsid sh -c \"echo \\$\\$ >> {p}; exec sleep 1001\" & wait",
        p = pids.display()
    );
    shim.ok("launch_app", json!({"app": foot_wrapper(env, &script)}));
    let pids = read_pids(&pids, 4);
    for pid in &pids {
        assert!(running(*pid), "{pid} isn't running: {}", cmdline(*pid));
    }
    pids
}

fn wait_all_gone(pids: &[u32], within: Duration) -> Result<Duration, String> {
    let start = Instant::now();
    if wait_until(within, || pids.iter().all(|p| !running(*p))) {
        return Ok(start.elapsed());
    }
    let left: Vec<String> = pids
        .iter()
        .filter(|p| running(**p))
        .map(|p| format!("{p} ({})", cmdline(*p)))
        .collect();
    Err(format!(
        "still running after {within:?}: {}",
        left.join(", ")
    ))
}

#[test]
fn kill_9_of_the_agent_ends_its_app_and_the_apps_children() {
    if !foot_or_skip() {
        return;
    }
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::ready(&env);
    let pids = foot_with_children(&env, &mut shim);
    let _reap = Reap(pids.clone());
    // foot, its shell and the shell's child (not the one that left the
    // session: see the next test).
    let in_session = &pids[..3];
    shim.kill_9();
    let gone = wait_all_gone(in_session, Duration::from_secs(2));
    assert!(gone.is_ok(), "{}", gone.unwrap_err());
}

#[test]
fn kill_9_of_the_agent_ends_even_a_process_its_app_started_with_setsid() {
    if !foot_or_skip() {
        return;
    }
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::ready(&env);
    let pids = foot_with_children(&env, &mut shim);
    let _reap = Reap(pids.clone());
    shim.kill_9();
    let gone = wait_all_gone(&pids, Duration::from_secs(2));
    assert!(gone.is_ok(), "{}", gone.unwrap_err());
}

#[test]
fn an_agent_that_disconnects_cleanly_leaves_no_app_behind() {
    if !foot_or_skip() {
        return;
    }
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::ready(&env);
    let pids = foot_with_children(&env, &mut shim);
    let _reap = Reap(pids.clone());
    shim.close_stdin();
    let gone = wait_all_gone(&pids, Duration::from_secs(3));
    assert!(gone.is_ok(), "{}", gone.unwrap_err());
}

#[test]
fn killing_one_agent_leaves_another_agents_app_running() {
    if !foot_or_skip() {
        return;
    }
    let env = Env::new();
    let mut daemon = Daemon::start(&env);
    let mut a = Shim::ready(&env);
    let mut b = Shim::ready(&env);
    let b_pids = env.path("b-pids");
    b.ok("launch_app", json!({"app": "foot", "args": ["sh", "-c", format!("echo $$ > '{}'; sleep 1000", b_pids.display())]}));
    let b_shell = read_pids(&b_pids, 1)[0];
    let a_pids = foot_with_children(&env, &mut a);
    let _reap = Reap(a_pids.iter().copied().chain([b_shell]).collect());

    a.kill_9();
    let gone = wait_all_gone(&a_pids[..3], Duration::from_secs(2));
    assert!(gone.is_ok(), "{}", gone.unwrap_err());
    std::thread::sleep(Duration::from_millis(500));
    assert!(running(b_shell), "the other agent's app died too");
    assert!(daemon.is_running());
    assert_eq!(b.screenshot().width, 1280);
}

#[test]
fn launching_an_app_that_isnt_installed_is_a_clear_error() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::ready(&env);
    let reply = shim.call("launch_app", json!({"app": "hc25-no-such-app"}));
    assert!(reply.is_error(), "{}", reply.raw);
    assert!(
        mentions(&reply.text(), "hc25-no-such-app"),
        "the error should name the app: {}",
        reply.raw
    );
    assert_eq!(shim.screenshot().width, 1280, "the session should carry on");
}

/// An app that exits without ever opening a window: the agent is told, and
/// the call doesn't hang.
#[test]
fn an_app_that_exits_without_a_window_is_an_error_not_a_hang() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::ready(&env);
    let tools = shim.request("tools/list", json!({}));
    assert!(
        tools.to_string().contains("\"launch_app\""),
        "no launch_app tool: {tools}"
    );
    let app = env.script("exits.sh", "exit 3");
    let started = Instant::now();
    let reply = shim.call("launch_app", json!({ "app": app }));
    assert!(reply.is_error(), "{}", reply.raw);
    assert!(!reply.text().trim().is_empty(), "{}", reply.raw);
    assert!(started.elapsed() < TIMEOUT);
    assert_eq!(shim.screenshot().width, 1280, "the session should carry on");
}
