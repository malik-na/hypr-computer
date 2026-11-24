//! Giving the app on the screen time for the agent's input. Long text goes in
//! parts, each once the app has read the last: all at once, it would fill
//! the app's connection faster than the app reads it, and the screen would
//! drop the app. A drag goes in parts too. After the input, the wait goes on until the screen shows the
//! app's response, so the agent's next screenshot has it: the app has read
//! the input, and then kept still for a moment, or drew nothing in response.

use std::time::{Duration, Instant};

use ports::{Action, Point, ScreenError};

use crate::off_screen;
use crate::process::{Pipe, ScreenChild};
use crate::protocol::Request;

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
/// How many moves a drag makes on its way.
const DRAG_STEPS: u32 = 10;

/// `action` as the requests it goes to the screen in, each sent once the app
/// has read the one before. A drag goes as a press, moves and a release, so
/// an app can start a drag while the button is down; both its ends are
/// checked before anything is pressed.
pub(crate) fn parts(action: Action) -> Result<Vec<Request>, ScreenError> {
    Ok(match action {
        Action::Type(text) if text.chars().count() > TYPE_PART => text
            .chars()
            .collect::<Vec<_>>()
            .chunks(TYPE_PART)
            .map(|part| Request::Act(Action::Type(part.iter().collect())))
            .collect(),
        Action::Drag { from, to } => {
            if let Some(why) = off_screen(from).or_else(|| off_screen(to)) {
                return Err(ScreenError(why));
            }
            let along = |a: u32, b: u32, step: u32| {
                let (a, b) = (i64::from(a), i64::from(b));
                (a + (b - a) * i64::from(step) / i64::from(DRAG_STEPS)) as u32
            };
            let moves = (1..=DRAG_STEPS).map(|step| {
                Request::Act(Action::Move(Point {
                    x: along(from.x, to.x, step),
                    y: along(from.y, to.y, step),
                }))
            });
            [Request::Press(from)]
                .into_iter()
                .chain(moves)
                .chain([Request::Release(to)])
                .collect()
        }
        action => vec![Request::Act(action)],
    })
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
