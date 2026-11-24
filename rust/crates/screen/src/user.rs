//! The daemon's side of a screen's user socket (`protocol::USER_SOCKET`):
//! the user's input in the live view comes in, and what the view is told
//! goes out, each in order.

use std::path::Path;

use ports::{UserInput, ViewNotice};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, mpsc};

use crate::protocol::{notice_line, parse_input};

pub(crate) struct UserLink {
    notices: mpsc::UnboundedSender<ViewNotice>,
    inputs: Mutex<mpsc::UnboundedReceiver<UserInput>>,
}

impl UserLink {
    /// Connects to the screen's user socket, and reads and writes it until
    /// either side closes it.
    pub(crate) async fn connect(path: &Path) -> std::io::Result<Self> {
        let (read, mut write) = UnixStream::connect(path).await?.into_split();
        let (notices, mut to_send) = mpsc::unbounded_channel::<ViewNotice>();
        tokio::spawn(async move {
            while let Some(notice) = to_send.recv().await {
                if write
                    .write_all(notice_line(notice).as_bytes())
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
        let (got, inputs) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(input) = parse_input(&line)
                    && got.send(input).is_err()
                {
                    return;
                }
            }
        });
        Ok(Self {
            notices,
            inputs: Mutex::new(inputs),
        })
    }

    pub(crate) async fn next(&self) -> Option<UserInput> {
        self.inputs.lock().await.recv().await
    }

    pub(crate) fn tell(&self, notice: ViewNotice) {
        // Fails only once the screen is gone, and then there is no view.
        let _ = self.notices.send(notice);
    }
}
