//! #62: once the daemon says it listens, SIGTERM stops it cleanly, however
//! soon the signal comes; and a daemon that died is replaced cleanly.
#![forbid(unsafe_code)]

mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::channel;
use std::thread;

use support::{Daemon, Env, Shim, TIMEOUT};

const ROUNDS: usize = 100;

/// A shell that sends SIGTERM to each pid written to it, with its builtin
/// `kill`: much sooner than starting a `kill` program for each.
struct Signaller {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Signaller {
    fn start() -> Self {
        let mut child = Command::new("sh")
            .args(["-c", r#"while read sig pid; do kill -"$sig" "$pid"; done"#])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        Signaller {
            stdin: child.stdin.take(),
            child,
        }
    }

    fn send(&mut self, signal: &str, pid: u32) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{signal} {pid}").unwrap();
        stdin.flush().unwrap();
    }
}

impl Drop for Signaller {
    fn drop(&mut self) {
        self.stdin = None;
        let _ = self.child.wait();
    }
}

/// Starts the daemon and sends it `signal` the moment its "listening on"
/// line arrives. Returns how it exited and its whole log.
fn signal_at_listening(env: &Env, signaller: &mut Signaller, signal: &str) -> (ExitStatus, String) {
    let mut child = env
        .command(&support::daemon_bin())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut log = String::new();
    loop {
        let mut line = String::new();
        if stderr.read_line(&mut line).unwrap() == 0 {
            let status = child.wait().unwrap();
            panic!("the daemon exited ({status}) before listening:\n{log}");
        }
        log.push_str(&line);
        if line.contains("listening on") {
            break;
        }
    }
    signaller.send(signal, child.id());

    let (tx, rest) = channel();
    thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        let _ = tx.send(s);
    });
    let Some(status) = support::wait_exit(&mut child, TIMEOUT) else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("the daemon didn't exit after SIG{signal}:\n{log}");
    };
    log.push_str(&rest.recv_timeout(TIMEOUT).unwrap_or_default());
    (status, log)
}

#[test]
fn sigterm_right_after_listening_always_stops_the_daemon_cleanly() {
    let mut signaller = Signaller::start();
    for round in 1..=ROUNDS {
        let env = Env::new();
        let (status, log) = signal_at_listening(&env, &mut signaller, "TERM");
        assert!(
            status.success(),
            "round {round}: SIGTERM right after \"listening\" ended the daemon with {status}, not 0:\n{log}"
        );
        assert!(
            !env.socket().exists(),
            "round {round}: the daemon left its socket behind:\n{log}"
        );
    }
}

#[test]
fn a_daemon_that_died_is_always_replaced_cleanly() {
    let mut signaller = Signaller::start();
    for round in 1..=ROUNDS {
        let env = Env::new();
        // Every other round the predecessor is killed outright, leaving its
        // socket file; otherwise it is asked to stop right after listening.
        let signal = if round % 2 == 0 { "KILL" } else { "TERM" };
        let (status, log) = signal_at_listening(&env, &mut signaller, signal);
        if signal == "TERM" {
            assert!(
                status.success(),
                "round {round}: the predecessor didn't stop cleanly on SIGTERM ({status}):\n{log}"
            );
        }

        // Panics, with its log, if it exits instead of listening.
        let second = Daemon::spawn(&env, env.command(&support::daemon_bin()), "second.log");
        let log = second.log();
        assert!(!log.contains("already running"), "round {round}:\n{log}");

        let mut shim = Shim::start(&env);
        shim.initialize();
    }
}
