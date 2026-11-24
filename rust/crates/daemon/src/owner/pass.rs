//! The screen a tool call gets once the guard lets it through. It asks the
//! owner before every input, so a stop reaches an input in progress
//! (`domain::stop`, specs/Stop.tla), and it ends the call for the guard.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use domain::SessionId;
use domain::stop::{Effect, Event};
use ports::{
    Action, AppCommand, BoxFuture, InputGate, Launched, Png, Screen, ScreenError, ScreenProcess,
};
use tokio::sync::{mpsc, oneshot};

use super::Msg;
use super::stops::STOPPED;

pub(super) struct Pass {
    id: SessionId,
    screen: Arc<dyn ScreenProcess>,
    tx: mpsc::UnboundedSender<Msg>,
    /// The call was told to abort: it gives no more input.
    stopped: AtomicBool,
    /// The call has stepped its `Finish` or `Abort`: the session's call in
    /// progress is no longer this one, so it steps nothing more.
    over: AtomicBool,
}

impl Pass {
    pub(super) fn new(
        id: SessionId,
        screen: Arc<dyn ScreenProcess>,
        tx: mpsc::UnboundedSender<Msg>,
    ) -> Self {
        Self {
            id,
            screen,
            tx,
            stopped: AtomicBool::new(false),
            over: AtomicBool::new(false),
        }
    }

    /// Steps the call's `event` in the owner's `Stops`; whether it may give
    /// its input. `Effect::Abort` stops the call.
    async fn step(&self, event: fn(SessionId) -> Event<SessionId>) -> bool {
        if self.over.load(Ordering::SeqCst) {
            return false;
        }
        let (reply, answer) = oneshot::channel();
        if self.tx.send(Msg::Input(event(self.id), reply)).is_err() {
            return false;
        }
        let effects = answer.await.unwrap_or_default();
        if effects.contains(&Effect::Abort(self.id)) {
            self.stopped.store(true, Ordering::SeqCst);
        }
        effects.contains(&Effect::Give(self.id))
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// A stopped call has released what it held: it is over.
    async fn abort(&self) {
        if self.is_stopped() {
            self.step(Event::Abort).await;
            self.over.store(true, Ordering::SeqCst);
        }
    }

    fn refused(&self) -> ScreenError {
        ScreenError(STOPPED.to_owned())
    }
}

impl InputGate for Pass {
    fn ask(&self, holds: bool) -> BoxFuture<'_, bool> {
        let event = if holds { Event::Press } else { Event::Move };
        Box::pin(async move { !self.is_stopped() && self.step(event).await })
    }

    fn released(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.step(Event::Release).await;
        })
    }
}

impl Screen for Pass {
    fn screenshot(&self) -> BoxFuture<'_, Result<Png, ScreenError>> {
        Box::pin(async move {
            if self.is_stopped() {
                return Err(self.refused());
            }
            self.screen.screenshot().await
        })
    }

    fn act(&self, action: Action) -> BoxFuture<'_, Result<(), ScreenError>> {
        Box::pin(async move {
            if self.is_stopped() {
                return Err(self.refused());
            }
            let acted = self.screen.act_gated(action, self).await;
            self.abort().await;
            acted
        })
    }

    fn launch(&self, app: AppCommand) -> BoxFuture<'_, Result<Launched, ScreenError>> {
        Box::pin(async move {
            if self.is_stopped() {
                return Err(self.refused());
            }
            self.screen.launch(app).await
        })
    }

    fn end_call(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move {
            // A call that failed holding something can't release it any more:
            // its screen broke.
            for event in [Event::Release, Event::Finish] {
                if !self.is_stopped() {
                    self.step(event).await;
                }
            }
            self.abort().await;
            self.over.store(true, Ordering::SeqCst);
            self.is_stopped().then(|| STOPPED.to_owned())
        })
    }
}

impl Drop for Pass {
    /// A call that never ended (its task panicked, or nobody took the pass)
    /// mustn't keep its session's next calls waiting.
    fn drop(&mut self) {
        if !self.over.load(Ordering::SeqCst) {
            let _ = self.tx.send(Msg::Dropped(self.id));
        }
    }
}
