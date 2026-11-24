//! Chromium's DevTools protocol, over the pipe that `--remote-debugging-pipe`
//! opens: the browser reads commands on its fd 3 and writes replies and
//! events on its fd 4, each a JSON object ended by a NUL byte. A pipe, not
//! a port: nothing but the daemon can reach the browser this way.

use std::collections::HashMap;
use std::io::{PipeReader, PipeWriter};
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ports::ScreenError;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::pipe;
use tokio::sync::{broadcast, oneshot};
use tokio::time::timeout;

/// How long the browser may take to answer a command.
const ANSWER_TIME: Duration = Duration::from_secs(15);
/// How many events are kept for a listener that is behind.
const EVENTS: usize = 4096;
/// The largest message read; a page's events are far smaller.
const MAX_MESSAGE: usize = 64 << 20;

/// The event every listener gets once the browser has closed its end.
pub(crate) const CLOSED: &str = "hypr-computer.closed";

/// Something that happened in the browser, as it said.
#[derive(Clone, Debug)]
pub(crate) struct Event {
    pub method: String,
    pub params: Value,
    /// The page session it happened in, if any.
    pub session: Option<String>,
}

type Waiting = HashMap<u64, oneshot::Sender<Result<Value, String>>>;

/// The daemon's end of a browser's DevTools pipe.
pub(crate) struct DevTools {
    to: tokio::sync::Mutex<pipe::Sender>,
    waiting: Arc<Mutex<Option<Waiting>>>,
    events: broadcast::Sender<Event>,
    next_id: AtomicU64,
}

/// The browser's ends of the pipe: it reads `commands` as its fd 3 and
/// writes `replies` as its fd 4.
pub(crate) struct BrowserEnds {
    pub commands: OwnedFd,
    pub replies: OwnedFd,
}

impl DevTools {
    /// A pipe each way, and a task that reads what the browser says.
    pub(crate) fn open() -> Result<(Self, BrowserEnds), ScreenError> {
        let cant = |e: std::io::Error| ScreenError(format!("can't make a pipe: {e}"));
        let (commands, to): (PipeReader, PipeWriter) = std::io::pipe().map_err(cant)?;
        let (from, replies): (PipeReader, PipeWriter) = std::io::pipe().map_err(cant)?;
        let to = pipe::Sender::from_owned_fd(OwnedFd::from(to)).map_err(cant)?;
        let from = pipe::Receiver::from_owned_fd(OwnedFd::from(from)).map_err(cant)?;
        let waiting = Arc::new(Mutex::new(Some(Waiting::new())));
        let (events, _) = broadcast::channel(EVENTS);
        tokio::spawn(read(from, waiting.clone(), events.clone()));
        Ok((
            DevTools {
                to: tokio::sync::Mutex::new(to),
                waiting,
                events,
                next_id: AtomicU64::new(1),
            },
            BrowserEnds {
                commands: commands.into(),
                replies: replies.into(),
            },
        ))
    }

    /// Whether the browser has closed its end: it has exited.
    pub(crate) fn closed(&self) -> bool {
        self.waiting.lock().map(|w| w.is_none()).unwrap_or(true)
    }

    /// Events from now on.
    pub(crate) fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Sends a command, to the page `session` or to the browser, and waits
    /// for its result.
    pub(crate) async fn call(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, ScreenError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        let (answer, answered) = oneshot::channel();
        match self.waiting.lock().as_deref_mut() {
            Ok(Some(waiting)) => waiting.insert(id, answer),
            _ => return Err(gone()),
        };
        let mut bytes = message.to_string().into_bytes();
        bytes.push(0);
        let sent = async {
            let mut to = self.to.lock().await;
            to.write_all(&bytes).await?;
            to.flush().await
        };
        let unwait = || {
            if let Ok(Some(waiting)) = self.waiting.lock().as_deref_mut() {
                waiting.remove(&id);
            }
        };
        if sent.await.is_err() {
            unwait();
            return Err(gone());
        }
        match timeout(ANSWER_TIME, answered).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(why))) => Err(ScreenError(format!("the browser refused {method}: {why}"))),
            Ok(Err(_)) => Err(gone()),
            Err(_) => {
                unwait();
                Err(ScreenError(format!(
                    "the browser didn't answer {method} within {}s",
                    ANSWER_TIME.as_secs()
                )))
            }
        }
    }
}

fn gone() -> ScreenError {
    ScreenError("the browser has exited".to_owned())
}

/// Reads the browser's messages until it closes its end: answers go to who
/// is waiting for them, events to every listener. Then nothing is waited
/// for any more.
async fn read(
    mut from: pipe::Receiver,
    waiting: Arc<Mutex<Option<Waiting>>>,
    events: broadcast::Sender<Event>,
) {
    let mut buffer = Vec::new();
    let mut chunk = vec![0; 64 << 10];
    while let Ok(n @ 1..) = from.read(&mut chunk).await {
        buffer.extend_from_slice(&chunk[..n]);
        while let Some(end) = buffer.iter().position(|b| *b == 0) {
            let message: Vec<u8> = buffer.drain(..=end).collect();
            let Ok(message) = serde_json::from_slice::<Value>(&message[..end]) else {
                continue;
            };
            deliver(message, &waiting, &events);
        }
        if buffer.len() > MAX_MESSAGE {
            break;
        }
    }
    if let Ok(mut waiting) = waiting.lock() {
        *waiting = None;
    }
    let _ = events.send(Event {
        method: CLOSED.to_owned(),
        params: Value::Null,
        session: None,
    });
}

fn deliver(message: Value, waiting: &Mutex<Option<Waiting>>, events: &broadcast::Sender<Event>) {
    if let Some(id) = message["id"].as_u64() {
        let answer = waiting
            .lock()
            .ok()
            .and_then(|mut w| w.as_mut().and_then(|w| w.remove(&id)));
        if let Some(answer) = answer {
            let result = match message.get("error") {
                Some(error) => Err(error["message"].as_str().unwrap_or("").to_owned()),
                None => Ok(message["result"].clone()),
            };
            let _ = answer.send(result);
        }
    } else if let Some(method) = message["method"].as_str() {
        let _ = events.send(Event {
            method: method.to_owned(),
            params: message["params"].clone(),
            session: message["sessionId"].as_str().map(str::to_owned),
        });
    }
}
