//! Starting an app on a screen. The screen starts it (see
//! `compositor/apps.rs`), so the app and all it starts are below the screen,
//! where the screen's teardown ends them.

use std::fs;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use ports::{AppCommand, Launched, ScreenError};

use crate::process::ScreenChild;
use crate::protocol::{Request, app_log};

/// How long an app may take to open its window before `launch` stops waiting.
const WINDOW_TIME: Duration = Duration::from_secs(10);
/// How often to look for the window meanwhile.
const POLL: Duration = Duration::from_millis(10);
/// How much of what an app printed is shown when it ends at once.
const OUTPUT_TAIL: usize = 2000;

impl ScreenChild {
    pub(crate) async fn start_app(&self, app: AppCommand) -> Result<Launched, ScreenError> {
        let before = self.status().await?.windows;
        let n = self.launched.fetch_add(1, Ordering::Relaxed) + 1;
        let pid = self
            .launched_pid(Request::Launch {
                n,
                program: app.program,
                args: app.args,
            })
            .await?;

        let start = Instant::now();
        loop {
            if let Some(status) = self.app_ended(pid).await? {
                let output = fs::read(self.dir.join(app_log(n))).unwrap_or_default();
                return Ok(Launched::Exited {
                    status,
                    output: tail(&output),
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

/// The end of `output`, as text.
fn tail(output: &[u8]) -> String {
    let start = output.len().saturating_sub(OUTPUT_TAIL);
    String::from_utf8_lossy(&output[start..]).trim().to_owned()
}
