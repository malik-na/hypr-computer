//! Starting an app on a screen. It joins the screen's process group, so the
//! screen's teardown ends it. It gets this screen as its display and nothing
//! of the user's session: no other display, no session bus, and a runtime dir
//! of the screen's own in place of the user's, whose sockets (the bus,
//! Hyprland's, the portals') lead to the user's desktop. It is told to use
//! the daemon's filtering proxy (`http_proxy` and the like).

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fs::DirBuilder;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ports::{AppCommand, Launched, ScreenError};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::apps::name_of;
use crate::process::ScreenChild;

/// How long an app may take to open its window before `launch` stops waiting.
const WINDOW_TIME: Duration = Duration::from_secs(10);
/// How long a launcher that ended well is given for the app it started to
/// open a window.
const AFTER_LAUNCHER: Duration = Duration::from_secs(3);
/// How often to look for the window meanwhile.
const POLL: Duration = Duration::from_millis(10);
/// How much of what an app printed is kept, to show if it ends at once.
const OUTPUT_TAIL: usize = 2000;
/// How long an app that ended may take to hand over the last of its output.
const DRAIN_TIME: Duration = Duration::from_millis(100);
/// The variables that tell programs which proxy to use, set to the daemon's
/// filtering proxy for every app.
const PROXY_VARS: &[&str] = &[
    "http_proxy",
    "https_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "all_proxy",
    "ALL_PROXY",
];
/// The variables that would let programs go around it.
const NO_PROXY_VARS: &[&str] = &["no_proxy", "NO_PROXY"];
/// What would lead an app to the user's desktop instead of this screen.
const USERS_SESSION: &[&str] = &[
    "DISPLAY",
    "WAYLAND_SOCKET",
    "DBUS_SESSION_BUS_ADDRESS",
    "HYPRLAND_INSTANCE_SIGNATURE",
];

impl ScreenChild {
    pub(crate) async fn start_app(&self, app: AppCommand) -> Result<Launched, ScreenError> {
        let command = self.app_command(&app.program, &app.args)?;
        self.run_app(command, &name_of(&app.program)).await
    }

    /// A command that starts `program` as an app on this screen, with this
    /// screen as its display and nothing of the user's session.
    pub(crate) fn app_command(
        &self,
        program: impl AsRef<OsStr>,
        args: &[String],
    ) -> Result<Command, ScreenError> {
        let runtime = self.app_runtime_dir();
        match DirBuilder::new().mode(0o700).create(&runtime) {
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => {
                return Err(ScreenError(e.to_string()));
            }
            _ => {}
        }
        let mut command = Command::new(program);
        command
            .args(args)
            .env("WAYLAND_DISPLAY", self.socket())
            .env("XDG_RUNTIME_DIR", &runtime)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(self.group.as_raw_nonzero().get());
        for name in USERS_SESSION.iter().chain(NO_PROXY_VARS) {
            command.env_remove(name);
        }
        // Programs that honour these go through the user's site limits;
        // programs that ignore them aren't limited.
        let proxy = format!("http://{}", self.proxy);
        for name in PROXY_VARS {
            command.env(name, &proxy);
        }
        Ok(command)
    }

    /// Starts `command`, made by [`Self::app_command`], as the app `name`,
    /// and waits a while for its window.
    pub(crate) async fn run_app(
        &self,
        mut command: Command,
        name: &str,
    ) -> Result<Launched, ScreenError> {
        let before = self.status().await?;
        let mut child = command.spawn().map_err(|e| ScreenError(e.to_string()))?;
        // The command holds the parent's copies of what it hands the app.
        drop(command);
        let Some(pid) = child.id() else {
            return Err(ScreenError("it vanished as it started".to_owned()));
        };
        self.apps.started(pid, name);
        let output = Output::default();
        let readers = [
            child.stdout.take().map(|out| output.keep(out)),
            child.stderr.take().map(|err| output.keep(err)),
        ];
        let (exited, mut exit) = oneshot::channel();
        tokio::spawn(async move {
            let _ = exited.send(child.wait().await);
        });

        let start = Instant::now();
        let mut ended = None;
        loop {
            let now = self.status().await?;
            if now.refused > before.refused {
                return Ok(Launched::PasswordManager);
            }
            if now.windows > before.windows {
                return Ok(Launched::Window { pid });
            }
            if ended.is_none()
                && let Ok(status) = exit.try_recv()
            {
                ended = Some((status, Instant::now()));
            }
            match &ended {
                Some((status, at)) => {
                    // A launcher that ended well may have started the app.
                    let failed = !status.as_ref().is_ok_and(ExitStatus::success);
                    if failed || at.elapsed() >= AFTER_LAUNCHER {
                        // The screen refuses a password manager's window as
                        // it ends the app, which may be what ended it.
                        if self.status().await?.refused > before.refused {
                            return Ok(Launched::PasswordManager);
                        }
                        for reader in readers.into_iter().flatten() {
                            let _ = tokio::time::timeout(DRAIN_TIME, reader).await;
                        }
                        return Ok(Launched::Exited {
                            status: describe(status),
                            failed,
                            output: output.text(),
                        });
                    }
                }
                None if start.elapsed() >= WINDOW_TIME => {
                    return Ok(Launched::NoWindow {
                        pid,
                        waited: WINDOW_TIME,
                    });
                }
                None => {}
            }
            tokio::time::sleep(POLL).await;
        }
    }
}

fn describe(status: &io::Result<ExitStatus>) -> String {
    match status {
        Ok(status) => status.to_string(),
        Err(e) => format!("its exit status is unknown: {e}"),
    }
}

/// The end of what an app prints, kept in memory, so an app that prints a
/// lot for hours fills nothing up.
#[derive(Clone, Default)]
struct Output(Arc<Mutex<VecDeque<u8>>>);

impl Output {
    /// Reads `from` to its end, keeping the last [`OUTPUT_TAIL`] bytes.
    fn keep(&self, mut from: impl AsyncRead + Unpin + Send + 'static) -> JoinHandle<()> {
        let output = self.clone();
        tokio::spawn(async move {
            let mut buffer = [0; 4096];
            while let Ok(n @ 1..) = from.read(&mut buffer).await {
                if let Ok(mut tail) = output.0.lock() {
                    tail.extend(&buffer[..n]);
                    let over = tail.len().saturating_sub(OUTPUT_TAIL);
                    tail.drain(..over);
                }
            }
        })
    }

    fn text(&self) -> String {
        let Ok(tail) = self.0.lock() else {
            return String::new();
        };
        let bytes: Vec<u8> = tail.iter().copied().collect();
        String::from_utf8_lossy(&bytes).trim().to_owned()
    }
}
