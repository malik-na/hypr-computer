//! Giving the app on the screen time for the agent's input. Long text goes in
//! parts, each once the app has read the last: all at once, it would fill
//! the app's connection faster than the app reads it, and the screen would
//! drop the app. After the input, the wait goes on until the screen shows the
//! app's response, so the agent's next screenshot has it: the app has read
//! the input, and then kept still for a moment, or drew nothing in response.

use std::time::{Duration, Instant};

use ports::{Action, ScreenError};

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
/// How many characters of text go to the app at a time: with Shift, about
/// 15 kB of key events, which any app's connection holds.
const TYPE_PART: usize = 100;

/// `action` in the parts it goes to the app in.
pub(crate) fn parts(action: Action) -> Vec<Action> {
    match action {
        Action::Type(text) if text.chars().count() > TYPE_PART => text
            .chars()
            .collect::<Vec<_>>()
            .chunks(TYPE_PART)
            .map(|part| Action::Type(part.iter().collect()))
            .collect(),
        action => vec![action],
    }
}

impl ScreenChild {
    /// Waits until the app has read the input given so far.
    pub(crate) async fn wait_read(&self, pipe: &mut Option<Pipe>) -> Result<(), ScreenError> {
        let start = Instant::now();
        while self.status_on(pipe).await?.unread && start.elapsed() < READ_LIMIT {
            tokio::time::sleep(POLL).await;
        }
        Ok(())
    }

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
