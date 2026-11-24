//! The one task that owns the sessions. It moves each through its lifecycle
//! (`domain::lifecycle`, specs/SessionLifecycle.tla): connections and screen
//! tasks send it what happened, `step` decides, and it does the effects.
//!
//! A connection holds a [`Ticket`] that ends its session when dropped, so a
//! session ends however its connection ends: closed, broken, or a panic.

use std::future::Future;
use std::sync::Arc;

use domain::lifecycle::{Effect, Event, SessionState};
use domain::{EndReason, SessionId, Sessions};
use ports::{BoxFuture, Clock, Screen, ScreenProcess, Screens};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tools::Admit;
use tracing::info;

enum Msg {
    Connect(CancellationToken, oneshot::Sender<SessionId>),
    Disconnect(SessionId, EndReason),
    ToolCall(SessionId, oneshot::Sender<Admission>),
    Screen(SessionId, News),
    Stop,
}

/// The answer to a tool call: the screen to run it on, or why not.
type Admission = Result<Arc<dyn Screen>, String>;

/// What a screen task saw.
enum News {
    Started,
    Ready,
    Failed(String),
    Exited,
    TornDown,
}

/// What the owner keeps for each session.
struct Live {
    /// Cancelling it closes the agent's connection.
    connection: CancellationToken,
    /// Set from `Effect::SpawnScreen` on, unless the spawn failed.
    screen: Option<Arc<dyn ScreenProcess>>,
    /// Why the session is being torn down; set when the teardown starts.
    ending: Option<EndReason>,
}

/// A handle to the owner. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Owner {
    tx: mpsc::UnboundedSender<Msg>,
}

impl Owner {
    /// Starts the owner. It runs until [`Owner::stop`] and then until every
    /// session is gone.
    pub fn spawn(clock: Arc<dyn Clock>, screens: Arc<dyn Screens>) -> (Owner, JoinHandle<()>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let state = State {
            sessions: Sessions::new(),
            clock,
            screens,
            tx: tx.clone(),
        };
        (Owner { tx }, tokio::spawn(run(rx, state)))
    }

    /// Opens a session for a new connection, which `connection` closes;
    /// `None` only if the owner has stopped.
    pub async fn open(&self, connection: CancellationToken) -> Option<Ticket> {
        let (reply, answer) = oneshot::channel();
        self.tx.send(Msg::Connect(connection, reply)).ok()?;
        let id = answer.await.ok()?;
        Some(Ticket {
            id,
            reason: EndReason::Disconnected,
            tx: self.tx.clone(),
        })
    }

    /// No more sessions will open: the owner ends once the last one is gone.
    pub fn stop(&self) {
        let _ = self.tx.send(Msg::Stop);
    }
}

/// One open session. Dropping it ends the session.
#[derive(Debug)]
pub struct Ticket {
    id: SessionId,
    reason: EndReason,
    tx: mpsc::UnboundedSender<Msg>,
}

impl Ticket {
    /// What the session's tool calls ask before they run.
    pub fn gate(&self) -> Gate {
        Gate {
            id: self.id,
            tx: self.tx.clone(),
        }
    }

    /// Ends the session, saying why.
    pub fn end(mut self, reason: EndReason) {
        self.reason = reason;
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let reason = std::mem::replace(&mut self.reason, EndReason::Disconnected);
        // Fails only if the owner is gone, and then there is nothing to end.
        let _ = self.tx.send(Msg::Disconnect(self.id, reason));
    }
}

/// Lets one session's tool calls through, as its lifecycle allows.
pub struct Gate {
    id: SessionId,
    tx: mpsc::UnboundedSender<Msg>,
}

impl Admit for Gate {
    fn admit(&self) -> BoxFuture<'_, Admission> {
        Box::pin(async move {
            let (reply, answer) = oneshot::channel();
            let stopped = || "hypr-computer's daemon is stopping.".to_owned();
            self.tx
                .send(Msg::ToolCall(self.id, reply))
                .map_err(|_| stopped())?;
            answer.await.map_err(|_| stopped())?
        })
    }
}

struct State {
    sessions: Sessions<Live>,
    clock: Arc<dyn Clock>,
    screens: Arc<dyn Screens>,
    /// For the screen tasks to report back on.
    tx: mpsc::UnboundedSender<Msg>,
}

async fn run(mut rx: mpsc::UnboundedReceiver<Msg>, mut state: State) {
    let mut stopping = false;
    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Connect(connection, reply) => state.connect(connection, reply),
            Msg::Disconnect(id, reason) => state.event(id, Event::Disconnect, Some(reason)),
            Msg::ToolCall(id, reply) => {
                let _ = reply.send(state.tool_call(id));
            }
            Msg::Screen(id, news) => state.news(id, news),
            Msg::Stop => stopping = true,
        }
        if stopping && state.sessions.open_count() == 0 {
            return;
        }
    }
}

impl State {
    fn connect(&mut self, connection: CancellationToken, reply: oneshot::Sender<SessionId>) {
        let live = Live {
            connection,
            screen: None,
            ending: None,
        };
        let (id, effect) = self.sessions.connect(self.clock.now(), live);
        info!("{id} started; {} open", self.sessions.open_count());
        let left = reply.send(id).is_err();
        self.act(id, effect);
        if left {
            // The connection went away while waiting: end it at once.
            self.event(id, Event::Disconnect, Some(EndReason::Disconnected));
        }
    }

    fn tool_call(&mut self, id: SessionId) -> Admission {
        let state = self.sessions.state(id);
        let effect = self
            .sessions
            .step(id, Event::ToolCall, self.clock.now())
            .and_then(|stepped| stepped.effect);
        let screen = self
            .sessions
            .data_mut(id)
            .and_then(|live| live.screen.clone());
        match (effect, screen) {
            (Some(Effect::ServeToolCall), Some(screen)) => Ok(screen),
            _ => Err(refusal(state)),
        }
    }

    fn news(&mut self, id: SessionId, news: News) {
        match news {
            News::Started => self.event(id, Event::ScreenStarted, None),
            News::Ready => {
                if self.sessions.state(id) == Some(SessionState::Verifying) {
                    info!("{id}: screen ready");
                }
                self.event(id, Event::ScreenReady, None);
            }
            News::Failed(why) => {
                self.event(id, Event::ScreenFailed, Some(EndReason::ScreenFailed(why)))
            }
            News::Exited => self.event(id, Event::ScreenCrashed, Some(EndReason::ScreenExited)),
            News::TornDown => self.event(id, Event::TeardownDone, None),
        }
    }

    /// Steps session `id` with `event` and does what that says. `why` is why
    /// the session ends, if this event starts its teardown.
    fn event(&mut self, id: SessionId, event: Event, why: Option<EndReason>) {
        // An event for a session that is gone is ignored.
        let Some(stepped) = self.sessions.step(id, event, self.clock.now()) else {
            return;
        };
        if let Some(gone) = stepped.gone {
            // Effect::CloseConnection: harmless if the agent already disconnected.
            gone.data.connection.cancel();
            let reason = gone.data.ending.unwrap_or(EndReason::Disconnected);
            info!(
                "{id} ended ({reason}) after {:.2}s; {} open",
                gone.lasted.as_secs_f64(),
                self.sessions.open_count()
            );
            return;
        }
        if stepped.effect == Some(Effect::Teardown)
            && let Some(live) = self.sessions.data_mut(id)
        {
            live.ending = why;
        }
        self.act(id, stepped.effect);
    }

    fn act(&mut self, id: SessionId, effect: Option<Effect>) {
        let screen = self
            .sessions
            .data_mut(id)
            .and_then(|live| live.screen.clone());
        match (effect, screen) {
            (Some(Effect::SpawnScreen), _) => match self.screens.spawn(id) {
                Ok(screen) => {
                    info!("{id}: starting its screen (pid {})", screen.pid());
                    if let Some(live) = self.sessions.data_mut(id) {
                        live.screen = Some(screen.clone());
                    }
                    let started = screen.clone();
                    self.report(id, async move {
                        match started.started().await {
                            Ok(()) => News::Started,
                            Err(e) => News::Failed(e.to_string()),
                        }
                    });
                    self.report(id, async move {
                        screen.exited().await;
                        News::Exited
                    });
                }
                Err(e) => self.event(
                    id,
                    Event::ScreenFailed,
                    Some(EndReason::ScreenFailed(e.to_string())),
                ),
            },
            (Some(Effect::VerifyScreen), Some(screen)) => self.report(id, async move {
                match screen.verify().await {
                    Ok(()) => News::Ready,
                    Err(e) => News::Failed(e.to_string()),
                }
            }),
            (Some(Effect::VerifyScreen), None) => self.event(
                id,
                Event::ScreenFailed,
                Some(EndReason::ScreenFailed("it has no process".to_owned())),
            ),
            (Some(Effect::Teardown), screen) => {
                if let Some(why) = self
                    .sessions
                    .data_mut(id)
                    .and_then(|live| live.ending.as_ref())
                {
                    info!("{id}: tearing down: {why}");
                }
                self.report(id, async move {
                    if let Some(screen) = screen {
                        screen.teardown().await;
                    }
                    News::TornDown
                });
            }
            // Tool calls are answered in `tool_call`, and the connection is
            // closed when the session is gone, in `event`.
            _ => {}
        }
    }

    /// Runs `task` and sends the owner what it saw. If the owner has
    /// stopped, every session is gone and the news no longer matters.
    fn report(&self, id: SessionId, task: impl Future<Output = News> + Send + 'static) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(Msg::Screen(id, task.await));
        });
    }
}

/// Why a tool call isn't served, for the agent.
fn refusal(state: Option<SessionState>) -> String {
    match state {
        Some(SessionState::Starting | SessionState::Verifying) => {
            "Your screen is still starting. Call the tool again in a moment.".to_owned()
        }
        _ => "Your session is ending and its screen is gone. Restart the hypr-computer \
              MCP server to get a new one."
            .to_owned(),
    }
}
