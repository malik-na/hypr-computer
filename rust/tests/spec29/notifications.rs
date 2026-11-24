//! A fake notification service: `org.freedesktop.Notifications` on a test's
//! private bus, as a desktop's notification daemon (mako, swaync, GNOME
//! Shell, Omarchy's) offers it. It records every call, keeps what it shows
//! "on screen", and lets the test answer a notification for the user.
//!
//! It shows a notification the moment it takes the `Notify` call, then
//! answers its id. It can instead fail every call, or hang: take calls and
//! answer none until [`Notifications::release`]. It can keep a notification
//! on screen after its sender closes it, as Omarchy's shell does.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use zbus::message::Type;
use zbus::names::BusName;
use zbus::{Connection, Message, MessageStream, OwnedVariant};

use crate::support::TIMEOUT;

pub const NAME: &str = "org.freedesktop.Notifications";
pub const PATH: &str = "/org/freedesktop/Notifications";
pub const INTERFACE: &str = "org.freedesktop.Notifications";

/// The first id the fake gives: not 1, so that a session number mistaken
/// for a notification id is noticed.
const FIRST_ID: u32 = 41;

/// How the service answers `Notify`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// It shows the notification and answers its id.
    Up,
    /// It answers every call with an error.
    Failing,
    /// It takes calls and answers none, until released.
    Hung,
}

/// One `Notify` call, as the service got it.
#[derive(Clone, Debug)]
pub struct Notify {
    pub app_name: String,
    pub replaces_id: u32,
    pub summary: String,
    pub body: String,
    /// Action keys and labels, as sent: key, label, key, label...
    pub actions: Vec<String>,
    pub expire_timeout: i32,
    /// The id the service gave it, once it has shown it.
    pub id: Option<u32>,
}

impl Notify {
    /// What the user reads: the summary and the body.
    pub fn text(&self) -> String {
        format!("{}\n{}", self.summary, self.body)
    }

    /// The action keys, without their labels.
    pub fn action_keys(&self) -> Vec<&str> {
        self.actions.iter().step_by(2).map(String::as_str).collect()
    }

    /// The label of action `key`.
    pub fn label(&self, key: &str) -> Option<&str> {
        let i = self.actions.iter().step_by(2).position(|k| k == key)?;
        self.actions.get(2 * i + 1).map(String::as_str)
    }

    /// Whether it goes away by itself, soon: it expires within 10 s, and has
    /// no buttons to wait for.
    pub fn is_short_lived(&self) -> bool {
        self.actions.is_empty() && (1..=10_000).contains(&self.expire_timeout)
    }
}

/// A call the service got.
#[derive(Clone, Debug)]
pub enum Call {
    Notify(Notify),
    Close(u32),
    /// Any other method, by name.
    Other(String),
}

struct State {
    mode: Mode,
    keeps_closed: bool,
    calls: Vec<Call>,
    /// On screen, by id: the last `Notify` shown under each id.
    screen: BTreeMap<u32, Notify>,
    next_id: u32,
    /// Notify calls taken while hung, with their index in `calls`.
    held: Vec<(Message, usize)>,
}

pub struct Notifications {
    state: Arc<Mutex<State>>,
    connection: Connection,
    runtime: tokio::runtime::Runtime,
}

impl Notifications {
    /// The service, on the bus at `address`, once it owns its name.
    pub fn start(address: &str, mode: Mode) -> Notifications {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let connection = runtime.block_on(async {
            let connection = zbus::connection::Builder::address(address)
                .build()
                .await
                .unwrap();
            connection.request_name(NAME).await.unwrap();
            connection
        });
        let state = Arc::new(Mutex::new(State {
            mode,
            keeps_closed: false,
            calls: Vec::new(),
            screen: BTreeMap::new(),
            next_id: FIRST_ID,
            held: Vec::new(),
        }));
        runtime.spawn(serve(connection.clone(), state.clone()));
        Notifications {
            state,
            connection,
            runtime,
        }
    }

    /// From now on it keeps a notification on screen after its sender
    /// closes it, as Omarchy's shell does.
    pub fn keep_closed(&self) {
        self.state.lock().unwrap().keeps_closed = true;
    }

    /// Every call it got, oldest first.
    pub fn calls(&self) -> Vec<Call> {
        self.state.lock().unwrap().calls.clone()
    }

    /// Every `Notify` call it got, oldest first, held ones included.
    pub fn notifies(&self) -> Vec<Notify> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::Notify(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    /// The ids its `CloseNotification` calls asked to close.
    pub fn closes(&self) -> Vec<u32> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::Close(id) => Some(id),
                _ => None,
            })
            .collect()
    }

    /// What is on screen, by id.
    pub fn on_screen(&self) -> BTreeMap<u32, Notify> {
        self.state.lock().unwrap().screen.clone()
    }

    /// Waits until `cond` holds of its `Notify` calls, and returns them.
    pub fn wait_for(&self, what: &str, cond: impl Fn(&[Notify]) -> bool) -> Vec<Notify> {
        let end = Instant::now() + TIMEOUT;
        loop {
            let notifies = self.notifies();
            if cond(&notifies) {
                return notifies;
            }
            assert!(
                Instant::now() < end,
                "the notification service never got {what}; it got {:#?}",
                self.calls()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Answers every held call, showing their notifications, and from now on
    /// answers at once.
    pub fn release(&self) {
        let held = {
            let mut state = self.state.lock().unwrap();
            state.mode = Mode::Up;
            std::mem::take(&mut state.held)
        };
        for (msg, index) in held {
            self.runtime
                .block_on(show(&self.connection, &self.state, &msg, index));
        }
    }

    /// The user presses the button of action `key` on notification `id`: the
    /// service says so, then takes the notification away, as one does
    /// without the "resident" hint.
    pub fn press(&self, id: u32, key: &str) {
        self.action_invoked(id, key);
        self.state.lock().unwrap().screen.remove(&id);
        self.closed(id, 2);
    }

    /// The user closes notification `id`.
    pub fn dismiss(&self, id: u32) {
        self.state.lock().unwrap().screen.remove(&id);
        self.closed(id, 2);
    }

    /// Says that the user invoked action `key` on notification `id`, whoever
    /// sent it.
    pub fn action_invoked(&self, id: u32, key: &str) {
        self.runtime
            .block_on(self.connection.emit_signal(
                None::<BusName<'_>>,
                PATH,
                INTERFACE,
                "ActionInvoked",
                &(id, key),
            ))
            .unwrap();
    }

    /// Says that notification `id` was closed, for `reason`.
    pub fn closed(&self, id: u32, reason: u32) {
        self.runtime
            .block_on(self.connection.emit_signal(
                None::<BusName<'_>>,
                PATH,
                INTERFACE,
                "NotificationClosed",
                &(id, reason),
            ))
            .unwrap();
    }
}

type NotifyArgs = (
    String,
    u32,
    String,
    String,
    String,
    Vec<String>,
    HashMap<String, OwnedVariant>,
    i32,
);

async fn serve(connection: Connection, state: Arc<Mutex<State>>) {
    let mut stream = MessageStream::from(&connection);
    while let Some(Ok(msg)) = stream.next().await {
        let header = msg.header();
        if header.message_type() != Type::MethodCall
            || header.interface().map(|i| i.as_str()) != Some(INTERFACE)
        {
            continue;
        }
        let member = header.member().map(|m| m.to_string()).unwrap_or_default();
        let _ = match member.as_str() {
            "Notify" => {
                let Ok(args) = msg.body().deserialize::<NotifyArgs>() else {
                    let _ = connection
                        .reply_error(
                            &header,
                            "org.freedesktop.DBus.Error.InvalidArgs",
                            &"bad args",
                        )
                        .await;
                    continue;
                };
                let (app_name, replaces_id, _icon, summary, body, actions, _hints, expire_timeout) =
                    args;
                let (mode, index) = {
                    let mut s = state.lock().unwrap();
                    s.calls.push(Call::Notify(Notify {
                        app_name,
                        replaces_id,
                        summary,
                        body,
                        actions,
                        expire_timeout,
                        id: None,
                    }));
                    (s.mode, s.calls.len() - 1)
                };
                match mode {
                    Mode::Up => {
                        show(&connection, &state, &msg, index).await;
                        Ok(())
                    }
                    Mode::Failing => {
                        connection
                            .reply_error(
                                &header,
                                "org.freedesktop.DBus.Error.Failed",
                                &"the fake notification service fails every call",
                            )
                            .await
                    }
                    Mode::Hung => {
                        state.lock().unwrap().held.push((msg.clone(), index));
                        Ok(())
                    }
                }
            }
            "CloseNotification" => {
                let id = msg.body().deserialize::<u32>().unwrap_or(0);
                let closed = {
                    let mut s = state.lock().unwrap();
                    s.calls.push(Call::Close(id));
                    !s.keeps_closed && s.screen.remove(&id).is_some()
                };
                let replied = connection.reply(&header, &()).await;
                if closed {
                    let _ = connection
                        .emit_signal(
                            None::<BusName<'_>>,
                            PATH,
                            INTERFACE,
                            "NotificationClosed",
                            &(id, 3u32),
                        )
                        .await;
                }
                replied
            }
            "GetCapabilities" => {
                state.lock().unwrap().calls.push(Call::Other(member));
                let caps: Vec<&str> = vec!["actions", "body", "persistence"];
                connection.reply(&header, &caps).await
            }
            "GetServerInformation" => {
                state.lock().unwrap().calls.push(Call::Other(member));
                connection
                    .reply(&header, &("fake", "hypr-computer tests", "0", "1.2"))
                    .await
            }
            _ => {
                state.lock().unwrap().calls.push(Call::Other(member));
                connection
                    .reply_error(
                        &header,
                        "org.freedesktop.DBus.Error.UnknownMethod",
                        &"not in the fake",
                    )
                    .await
            }
        };
    }
}

/// Shows the notification of `calls[index]`, in place of the one it
/// replaces, and answers `msg` with its id.
async fn show(connection: &Connection, state: &Mutex<State>, msg: &Message, index: usize) {
    let id = {
        let mut s = state.lock().unwrap();
        let Call::Notify(mut notify) = s.calls[index].clone() else {
            return;
        };
        // As the specification says: a replacement keeps the id it replaces,
        // and shows anew if that one is gone.
        let id = if notify.replaces_id != 0 {
            notify.replaces_id
        } else {
            s.next_id += 1;
            s.next_id - 1
        };
        notify.id = Some(id);
        s.screen.insert(id, notify.clone());
        s.calls[index] = Call::Notify(notify);
        id
    };
    let _ = connection.reply(&msg.header(), &id).await;
}
