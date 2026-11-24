//! The desktop's notification service: `org.freedesktop.Notifications` on
//! the D-Bus session bus (the Desktop Notifications Specification, 1.2).
//!
//! One task owns the bus connection. It sends each request as it comes and
//! reads everything the bus sends back from one stream, so what it hears
//! (answers to its calls, and in later changes the user's buttons) comes out
//! in the order the service sent it. Callers never wait for the service.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::num::NonZeroU32;
use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;
use std::time::Duration;

use domain::SessionId;
use futures_util::StreamExt;
use ports::{BoxFuture, Heard, Notification, Notifications};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::warn;
use zbus::Variant;
use zbus::message::{Flags, Type};
use zbus::{Connection, MatchRule, Message, MessageStream};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
const APP_NAME: &str = "hypr-computer";
const ICON: &str = "input-mouse";

/// How long connecting to the bus may take.
const CONNECT_TIME: Duration = Duration::from_secs(5);

/// The session bus's address: `$DBUS_SESSION_BUS_ADDRESS`, or else
/// `$XDG_RUNTIME_DIR/bus`, where systemd puts it.
pub fn session_bus(address: Option<&OsStr>, runtime_dir: Option<&OsStr>) -> Option<String> {
    if let Some(address) = address.filter(|a| !a.is_empty()) {
        return Some(address.to_string_lossy().into_owned());
    }
    let dir = runtime_dir.filter(|d| !d.is_empty())?;
    let mut path = dir.as_bytes().to_vec();
    path.extend_from_slice(b"/bus");
    Some(format!("unix:path={}", escape(&path)))
}

/// A D-Bus address value: bytes other than `[-0-9A-Za-z_/.\*]` as `%xx`.
fn escape(value: &[u8]) -> String {
    value
        .iter()
        .map(|&b| match b {
            b'-' | b'_' | b'/' | b'.' | b'\\' | b'*' => (b as char).to_string(),
            b if b.is_ascii_alphanumeric() => (b as char).to_string(),
            b => format!("%{b:02x}"),
        })
        .collect()
}

/// The notification service on the bus at `bus`; with no bus, every
/// notification fails.
pub fn notifications(bus: Option<String>) -> Arc<dyn Notifications> {
    let (requests, rx) = mpsc::unbounded_channel();
    let (heard_tx, heard) = mpsc::unbounded_channel();
    tokio::spawn(serve(bus, rx, heard_tx));
    Arc::new(DBusNotifications {
        requests,
        heard: Mutex::new(heard),
    })
}

enum Request {
    Show(SessionId, Notification),
    Send(Notification),
    /// Answered once every request before it is sent.
    Flush(oneshot::Sender<()>),
}

struct DBusNotifications {
    requests: mpsc::UnboundedSender<Request>,
    heard: Mutex<mpsc::UnboundedReceiver<Heard>>,
}

impl Notifications for DBusNotifications {
    fn show(&self, session: SessionId, notification: Notification) {
        // Fails only once the runtime is shutting down.
        let _ = self.requests.send(Request::Show(session, notification));
    }

    fn send(&self, notification: Notification) {
        let _ = self.requests.send(Request::Send(notification));
    }

    fn flush(&self) -> BoxFuture<'_, ()> {
        let (done, flushed) = oneshot::channel();
        let _ = self.requests.send(Request::Flush(done));
        Box::pin(async {
            let _ = flushed.await;
        })
    }

    fn heard(&self) -> BoxFuture<'_, Heard> {
        Box::pin(async {
            match self.heard.lock().await.recv().await {
                Some(heard) => heard,
                // The task is gone: nothing more will be heard.
                None => std::future::pending().await,
            }
        })
    }
}

async fn serve(
    bus: Option<String>,
    mut requests: mpsc::UnboundedReceiver<Request>,
    heard: mpsc::UnboundedSender<Heard>,
) {
    let connected = match &bus {
        None => Err(
            "there is no session bus: neither DBUS_SESSION_BUS_ADDRESS nor \
                     XDG_RUNTIME_DIR is set"
                .to_owned(),
        ),
        Some(address) => connect(address).await,
    };
    let (connection, mut stream) = match connected {
        Ok(connected) => connected,
        Err(why) => {
            warn!("no desktop notifications: {why}");
            return refuse(requests, heard, why).await;
        }
    };
    // The calls waiting for their answer, by serial.
    let mut waiting: HashMap<NonZeroU32, SessionId> = HashMap::new();
    loop {
        tokio::select! {
            request = requests.recv() => match request {
                None => return,
                Some(Request::Show(session, notification)) => {
                    match send(&connection, &notification, true).await {
                        Ok(serial) => {
                            waiting.insert(serial, session);
                        }
                        Err(why) => {
                            let _ = heard.send(Heard::Failed(session, why));
                        }
                    }
                }
                Some(Request::Send(notification)) => {
                    if let Err(why) = send(&connection, &notification, false).await {
                        warn!("a notification wasn't shown: {why}");
                    }
                }
                Some(Request::Flush(done)) => {
                    let _ = done.send(());
                }
            },
            message = stream.next() => match message {
                Some(Ok(message)) => {
                    if let Some(news) = hear(&message, &mut waiting) {
                        let _ = heard.send(news);
                    }
                }
                Some(Err(_)) => {}
                None => {
                    let why = "the session bus closed the connection".to_owned();
                    warn!("no more desktop notifications: {why}");
                    for (_, session) in waiting.drain() {
                        let _ = heard.send(Heard::Failed(session, why.clone()));
                    }
                    return refuse(requests, heard, why).await;
                }
            },
        }
    }
}

/// Answers every request with `why`, for as long as requests come.
async fn refuse(
    mut requests: mpsc::UnboundedReceiver<Request>,
    heard: mpsc::UnboundedSender<Heard>,
    why: String,
) {
    while let Some(request) = requests.recv().await {
        match request {
            Request::Show(session, _) => {
                let _ = heard.send(Heard::Failed(session, why.clone()));
            }
            Request::Send(_) => {}
            Request::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

async fn connect(address: &str) -> Result<(Connection, MessageStream), String> {
    let connecting = async {
        let connection = zbus::connection::Builder::address(address).build().await?;
        // Made before the first call: it gets every message from now on.
        let stream = MessageStream::from(&connection);
        // The service's signals: the user's buttons, and closed notices.
        let signals = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(NAME)
            .interface(INTERFACE)
            .path(PATH)
            .build()?;
        zbus::fdo::DBusProxy::new(&connection)
            .await?
            .add_match_rule(signals)
            .await?;
        Ok::<_, zbus::Error>((connection, stream))
    };
    match tokio::time::timeout(CONNECT_TIME, connecting).await {
        Ok(Ok(connected)) => Ok(connected),
        Ok(Err(e)) => Err(format!("can't reach the session bus at {address}: {e}")),
        Err(_) => Err(format!(
            "the session bus at {address} didn't answer in {}s",
            CONNECT_TIME.as_secs()
        )),
    }
}

/// Sends a `Notify` call, and returns its serial. Without `answer`, the
/// service is told not to answer it.
async fn send(
    connection: &Connection,
    n: &Notification,
    answer: bool,
) -> Result<NonZeroU32, String> {
    let actions: Vec<&str> = n
        .actions
        .iter()
        .flat_map(|(key, label)| [key.as_str(), label.as_str()])
        .collect();
    // Critical, as the Python server's: some services let a normal one
    // expire whatever it asks.
    let urgency: u8 = if n.expires_ms.is_none() { 2 } else { 1 };
    let hints = HashMap::from([("urgency", Variant::from(urgency))]);
    let expire = n
        .expires_ms
        .map_or(0, |ms| i32::try_from(ms).unwrap_or(i32::MAX));
    let body = (
        APP_NAME,
        n.replaces.unwrap_or(0),
        ICON,
        n.summary.as_str(),
        n.body.as_str(),
        actions,
        hints,
        expire,
    );
    let flags = if answer {
        Flags::empty()
    } else {
        Flags::NO_REPLY_EXPECTED
    };
    let message = Message::method_call(PATH, "Notify")
        .with_flags(flags)
        .destination(NAME)
        .interface(INTERFACE)
        .build(&body)
        .map_err(|e| format!("can't make the Notify call: {e}"))?;
    let serial = message.primary_header().serial_num();
    connection
        .send(&message)
        .await
        .map_err(|e| format!("can't send the Notify call: {e}"))?;
    Ok(serial)
}

/// What `message` says, if it is news.
fn hear(message: &Message, waiting: &mut HashMap<NonZeroU32, SessionId>) -> Option<Heard> {
    let header = message.header();
    match message.message_type() {
        Type::MethodReturn => {
            let session = waiting.remove(&header.reply_serial()?)?;
            Some(match message.body().deserialize::<u32>() {
                Ok(id) => Heard::Shown(session, id),
                Err(e) => Heard::Failed(session, format!("the service's answer is wrong: {e}")),
            })
        }
        Type::Error => {
            let session = waiting.remove(&header.reply_serial()?)?;
            let name = header
                .error_name()
                .map_or_else(String::new, |e| e.to_string());
            let detail = message.body().deserialize::<String>().unwrap_or_default();
            Some(Heard::Failed(session, format!("{name}: {detail}")))
        }
        Type::Signal if header.interface().is_some_and(|i| i.as_str() == INTERFACE) => {
            let body = message.body();
            match header.member()?.as_str() {
                "ActionInvoked" => {
                    let (id, key) = body.deserialize::<(u32, String)>().ok()?;
                    Some(Heard::Action(id, key))
                }
                "NotificationClosed" => {
                    let (id, _reason) = body.deserialize::<(u32, u32)>().ok()?;
                    Some(Heard::Closed(id))
                }
                _ => None,
            }
        }
        _ => None,
    }
}
