//! The `hypr-computer` commands: one per connection to the control socket,
//! answered by the owner (`domain::control` has the format).

use std::time::Duration;

use domain::control::{Command, Reply};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::time::timeout;
use tracing::warn;

use crate::owner::Owner;

/// How long a command may take to say what it wants.
const ASK_TIME: Duration = Duration::from_secs(5);
/// The longest a command's line may be; every command is a word or two.
const MAX_ASK: u64 = 1024;

/// Answers commands until the task is dropped.
pub async fn serve(listener: UnixListener, owner: Owner) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(answer(stream, owner.clone()));
            }
            Err(e) => {
                warn!("accept on the control socket failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn answer(stream: UnixStream, owner: Owner) {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    let asked = timeout(
        ASK_TIME,
        BufReader::new(read.take(MAX_ASK)).read_line(&mut line),
    )
    .await;
    let reply = match (asked, Command::parse(&line)) {
        (Ok(Ok(_)), Some(Command::Watch)) => owner.watch().await,
        (Ok(Ok(_)), None) => Some(Reply::Refused(format!(
            "unknown command {:?}",
            line.trim_end()
        ))),
        // It went away, or never said anything.
        _ => return,
    };
    let reply =
        reply.unwrap_or_else(|| Reply::Refused("hypr-computer's daemon is stopping.".to_owned()));
    let _ = write.write_all(reply.line().as_bytes()).await;
}
