//! #63: when the daemon goes away, the shim exits non-zero with one clear
//! line on stderr, whether it notices while reading from the daemon or while
//! writing to it. An agent's client must be able to tell the daemon went
//! away from the agent ending the session.
#![forbid(unsafe_code)]

mod support;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use support::{Daemon, Env, TIMEOUT};

/// The shim, driven over raw pipes so a test controls exactly when it reads
/// the shim's stdout and when it writes to its stdin.
struct RawShim {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
}

impl RawShim {
    fn start(env: &Env) -> Self {
        let mut child = env
            .command(&support::shim_bin())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        RawShim {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            child,
        }
    }

    /// Writes one message; false once the shim no longer reads its stdin.
    fn send(&mut self, message: &Value) -> bool {
        let Some(stdin) = self.stdin.as_mut() else {
            return false;
        };
        writeln!(stdin, "{message}")
            .and_then(|()| stdin.flush())
            .is_ok()
    }

    fn handshake(&mut self) {
        assert!(self.send(&json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "shim-exit-spec", "version": "0"}
            }
        })));
        assert!(self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})));
    }

    /// Reads the shim's stdout, one line at a time, on another thread.
    fn read_lines(&mut self) -> Receiver<String> {
        let stdout = self.stdout.take().unwrap();
        let (tx, rx) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        rx
    }

    /// What each of the shim's threads waits in, as the kernel names it.
    fn waits(&self) -> Vec<String> {
        let tasks = Path::new("/proc")
            .join(self.child.id().to_string())
            .join("task");
        fs::read_dir(tasks)
            .map(|dir| {
                dir.filter_map(|t| fs::read_to_string(t.ok()?.path().join("wchan")).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether one of the shim's threads is blocked writing to a full pipe.
    fn stuck_writing_stdout(&self) -> bool {
        self.waits().iter().any(|w| w.contains("pipe_write"))
    }

    /// Waits for the shim to exit; returns its status and its stderr.
    fn exit(&mut self, what: &str) -> (ExitStatus, String) {
        let Some(status) = support::wait_exit(&mut self.child, TIMEOUT) else {
            panic!("{what}: the shim didn't exit after the daemon stopped");
        };
        let mut stderr = String::new();
        if let Some(mut e) = self.child.stderr.take() {
            e.read_to_string(&mut stderr).unwrap();
        }
        (status, stderr)
    }
}

impl Drop for RawShim {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Waits for the answer with `id` on `lines`.
fn answer(lines: &Receiver<String>, id: u64) -> Value {
    loop {
        let line = lines
            .recv_timeout(TIMEOUT)
            .unwrap_or_else(|e| panic!("no answer to request {id}: {e}"));
        let msg: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("not JSON from the shim ({e}): {line}"));
        if msg["id"] == json!(id) {
            return msg;
        }
    }
}

/// What the shim must do once the daemon is gone: fail, with one clear line.
fn assert_daemon_gone(what: &str, status: ExitStatus, stderr: &str, daemon_log: &str) {
    assert!(
        !status.success(),
        "{what}: the shim exited 0 after the daemon stopped, as if the agent had ended the session; \
         stderr: {stderr:?}\ndaemon log:\n{daemon_log}"
    );
    assert_eq!(
        stderr.lines().count(),
        1,
        "{what}: the shim should say what happened in exactly one line on stderr: {stderr:?}"
    );
    assert!(
        stderr.contains("daemon"),
        "{what}: the shim's line should say the daemon went away: {stderr:?}"
    );
}

#[test]
fn the_shim_fails_when_its_write_to_a_stopped_daemon_fails() {
    // Deterministic: the agent doesn't read the shim's stdout for a while, so
    // the shim is stuck passing the daemon's answers on and only finds out the
    // daemon stopped when its next write to the daemon fails.
    for round in 1..=10 {
        let what = format!("round {round}");
        let env = Env::new();
        let mut daemon = Daemon::start(&env);
        let mut shim = RawShim::start(&env);
        shim.handshake();
        daemon.wait_for_log("session 1 started");
        // Ask for tool lists (over 3 KB each) without reading them until the
        // shim's stdout pipe is full and the shim is stuck writing to it. Stop
        // there, so the daemon itself isn't stuck writing to the shim.
        let mut id = 0;
        while !shim.stuck_writing_stdout() {
            id += 1;
            assert!(
                id <= 1000,
                "{what}: the shim never filled its stdout pipe; its threads wait in {:?}",
                shim.waits()
            );
            assert!(shim.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"})));
            thread::sleep(Duration::from_millis(5));
        }

        daemon.terminate();
        // The agent writes again; this write is the one that fails.
        shim.send(&json!({"jsonrpc": "2.0", "id": 1000, "method": "ping"}));
        thread::sleep(Duration::from_millis(300));
        // Now the agent catches up on the shim's stdout.
        let _lines = shim.read_lines();
        let (status, stderr) = shim.exit(&what);
        assert_daemon_gone(&what, status, &stderr, &daemon.log());
    }
}

#[test]
fn the_shim_fails_when_the_daemon_stops_while_the_agent_keeps_writing() {
    for round in 1..=20 {
        let what = format!("round {round}");
        let env = Env::new();
        let mut daemon = Daemon::start(&env);
        let mut shim = RawShim::start(&env);
        let lines = shim.read_lines();
        shim.handshake();
        answer(&lines, 0);

        // The agent sends pings as fast as it can while the daemon stops.
        let mut stdin = shim.stdin.take().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let writing = {
            let stop = stop.clone();
            thread::spawn(move || {
                let mut id = 1u64;
                while !stop.load(Ordering::Relaxed) {
                    let ping = json!({"jsonrpc": "2.0", "id": id, "method": "ping"});
                    if writeln!(stdin, "{ping}")
                        .and_then(|()| stdin.flush())
                        .is_err()
                    {
                        break;
                    }
                    id += 1;
                }
            })
        };
        thread::sleep(Duration::from_millis(20));
        daemon.terminate();
        let (status, stderr) = shim.exit(&what);
        stop.store(true, Ordering::Relaxed);
        writing.join().unwrap();
        assert_daemon_gone(&what, status, &stderr, &daemon.log());
    }
}

#[test]
fn the_shim_fails_when_the_daemon_stops_while_it_is_reading() {
    for round in 1..=20 {
        let what = format!("round {round}");
        let env = Env::new();
        let mut daemon = Daemon::start(&env);
        let mut shim = RawShim::start(&env);
        let lines = shim.read_lines();
        shim.handshake();
        answer(&lines, 0);
        assert!(shim.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})));
        answer(&lines, 1);

        // Everything the agent sent has been answered; the shim is waiting
        // on the daemon when it stops.
        daemon.terminate();
        let (status, stderr) = shim.exit(&what);
        assert_daemon_gone(&what, status, &stderr, &daemon.log());
    }
}
