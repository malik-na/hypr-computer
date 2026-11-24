//! The daemon accepts a socket passed in by systemd (LISTEN_FDS), without
//! needing systemd: the test plays systemd's part.
#![forbid(unsafe_code)]

mod support;

use std::fs;
use std::os::fd::OwnedFd;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixListener;
use std::process::Command;

use command_fds::{CommandFdExt, FdMapping};
use support::{Daemon, Env, Shim, daemon_bin};

#[test]
fn the_daemon_serves_a_socket_passed_by_systemd() {
    let env = Env::new();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(env.daemon_dir())
        .unwrap();
    let listener: OwnedFd = UnixListener::bind(env.socket()).unwrap().into();

    // As systemd does: the socket is fd 3, LISTEN_PID is the daemon's pid.
    // `sh` learns its pid, exports it, then `exec`s the daemon in its place.
    let mut cmd = Command::new("/bin/sh");
    cmd.env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", env.path("home"))
        .args(["-c", "export LISTEN_PID=$$ LISTEN_FDS=1; exec \"$0\""])
        .arg(daemon_bin())
        .fd_mappings(vec![FdMapping {
            parent_fd: listener,
            child_fd: 3,
        }])
        .unwrap();
    // No XDG_RUNTIME_DIR for the daemon: it can only be serving the passed socket.
    let mut daemon = Daemon::spawn(&env, cmd, "daemon.log");
    daemon.wait_for_log("listening on the socket passed by systemd");

    let mut shim = Shim::start(&env);
    let init = shim.initialize();
    assert_eq!(init["serverInfo"]["name"], "hypr-computer");
    assert_eq!(shim.screenshot().width, 1280);

    assert!(daemon.terminate().success(), "{}", daemon.log());
    assert!(
        env.socket().exists(),
        "the socket belongs to systemd; the daemon must not remove it"
    );
}
