//! A private D-Bus session bus for one test: its own `dbus-daemon`, on a
//! socket in the test's dir, with a config of our own. It has no service
//! dirs, so nothing on it is ever activated: no real notification service
//! can start because a test called one. It never is the user's session bus.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub struct Bus {
    child: Child,
    socket: PathBuf,
    /// What `DBUS_SESSION_BUS_ADDRESS` is set to, to use it.
    pub address: String,
}

impl Bus {
    /// A bus listening on `socket`, which must not exist yet.
    pub fn start(socket: &Path) -> Bus {
        let config = socket.with_extension("conf");
        fs::write(
            &config,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
                socket.display()
            ),
        )
        .unwrap();
        let mut child = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--nopidfile", "--print-address=1"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|e| {
                panic!(
                    "couldn't run dbus-daemon ({e}): the notice's tests need it \
                     (Debian and Ubuntu: dbus-daemon; Arch: dbus)"
                )
            });
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let address = line.trim().to_owned();
        assert!(
            address.starts_with("unix:"),
            "dbus-daemon didn't start: it printed {line:?}"
        );
        Bus {
            child,
            socket: socket.to_owned(),
            address,
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
