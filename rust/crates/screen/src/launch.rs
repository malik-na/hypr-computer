//! Starting an app on a screen. It joins the screen's process group, so the
//! screen's teardown ends it, and it gets this screen as its display and none
//! of the user's.

use std::fs::{self, File};
use std::io;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use ports::{AppCommand, Launched, ScreenError};
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::process::ScreenChild;

/// How long an app may take to open its window before `launch` stops waiting.
const WINDOW_TIME: Duration = Duration::from_secs(10);
/// How often to look for the window meanwhile.
const POLL: Duration = Duration::from_millis(10);
/// How much of what an app printed is shown when it ends at once.
const OUTPUT_TAIL: usize = 2000;
/// What would lead an app to the user's desktop instead of this screen.
const USERS_DESKTOP: &[&str] = &["DISPLAY", "WAYLAND_SOCKET", "HYPRLAND_INSTANCE_SIGNATURE"];

impl ScreenChild {
    pub(crate) async fn start_app(&self, app: AppCommand) -> Result<Launched, ScreenError> {
        let before = self.status().await?.windows;
        let n = self.launched.fetch_add(1, Ordering::Relaxed) + 1;
        // What it prints, kept with the screen's files, so it goes with them.
        let output = self.dir.join(format!("app-{n}.log"));
        let cant = |e: io::Error| ScreenError(format!("{e}"));
        let log = File::create(&output).map_err(cant)?;
        let mut command = Command::new(&app.program);
        command
            .args(&app.args)
            .env("WAYLAND_DISPLAY", self.socket())
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(cant)?)
            .stderr(log)
            .process_group(self.group.as_raw_nonzero().get());
        for name in USERS_DESKTOP {
            command.env_remove(name);
        }
        let mut child = command.spawn().map_err(cant)?;
        let Some(pid) = child.id() else {
            return Err(ScreenError("it vanished as it started".to_owned()));
        };
        let (exited, mut exit) = oneshot::channel();
        tokio::spawn(async move {
            let _ = exited.send(child.wait().await);
        });

        let start = Instant::now();
        loop {
            if let Ok(status) = exit.try_recv() {
                return Ok(Launched::Exited {
                    status: describe(status),
                    output: tail(&fs::read(&output).unwrap_or_default()),
                });
            }
            if self.status().await?.windows > before {
                return Ok(Launched::Window { pid });
            }
            if start.elapsed() >= WINDOW_TIME {
                return Ok(Launched::NoWindow {
                    pid,
                    waited: WINDOW_TIME,
                });
            }
            tokio::time::sleep(POLL).await;
        }
    }
}

fn describe(status: io::Result<ExitStatus>) -> String {
    match status {
        Ok(status) => status.to_string(),
        Err(e) => format!("its exit status is unknown: {e}"),
    }
}

/// The end of `output`, as text.
fn tail(output: &[u8]) -> String {
    let start = output.len().saturating_sub(OUTPUT_TAIL);
    String::from_utf8_lossy(&output[start..]).trim().to_owned()
}
