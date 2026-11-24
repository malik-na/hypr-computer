//! Giving the app on the screen time for the agent's input: the wait goes on
//! until the screen shows the app's response, so the agent's next screenshot
//! has it. The app has read the input, and then kept still for a moment, or
//! drew nothing in response.

use std::time::{Duration, Instant};

use ports::ScreenError;

use crate::process::{Pipe, ScreenChild};

/// The longest wait for the app to read the input, for an app that hangs.
const READ_LIMIT: Duration = Duration::from_secs(3);
/// How long the screen must keep still once the app has read it.
const STILL: Duration = Duration::from_millis(50);
/// How long an app that drew nothing yet has to start drawing.
const REACT: Duration = Duration::from_millis(200);
/// The longest wait once it has read it, for a screen that never keeps still.
const DRAW_LIMIT: Duration = Duration::from_secs(1);
/// How often to look meanwhile.
const POLL: Duration = Duration::from_millis(5);
impl ScreenChild {
    /// Waits until the screen has settled after an input given when apps had
    /// drawn `since` times.
    pub(crate) async fn settle(
        &self,
        pipe: &mut Option<Pipe>,
        since: u64,
    ) -> Result<(), ScreenError> {
        let start = Instant::now();
        let mut read = None;
        loop {
            let status = self.status_on(pipe).await?;
            let now = Instant::now();
            if !status.unread {
                read.get_or_insert(now);
            }
            let settled = match read {
                None => now - start >= READ_LIMIT,
                Some(read) => {
                    let still = status.idle.min(now - read);
                    let drew = status.commits > since;
                    (still >= STILL && (drew || now - read >= REACT)) || now - read >= DRAW_LIMIT
                }
            };
            if settled {
                return Ok(());
            }
            tokio::time::sleep(POLL).await;
        }
    }
}
