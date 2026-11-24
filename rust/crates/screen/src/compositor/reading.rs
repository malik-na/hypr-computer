//! Knowing when the app on the screen has read the agent's input. After each
//! input the app gets a ping, which it answers only once it has handled every
//! event sent before it. Until then its picture may not show the input yet.

use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::shell::xdg::ShellClient;

use crate::state::State;

#[derive(Default)]
pub struct Reading {
    /// Inputs given, and how many of them apps are known to have read.
    given: u64,
    read: u64,
    /// The ping in flight, and how many inputs had been given when it went.
    ping: Option<(ShellClient, u64)>,
}

impl State {
    /// Notes an input given to the app on the screen.
    pub fn input_given(&mut self) {
        self.reading.given += 1;
        self.ping_shown();
    }

    /// Whether an app may not have read all the input yet.
    pub fn input_unread(&self) -> bool {
        let Reading { given, read, ping } = &self.reading;
        read < given && ping.as_ref().is_some_and(|(app, _)| app.alive())
    }

    /// The app answered a ping.
    pub fn ponged(&mut self, app: ShellClient) {
        let Some((_, given)) = self.reading.ping.take_if(|(pinged, _)| *pinged == app) else {
            return;
        };
        self.reading.read = given;
        if self.reading.read < self.reading.given {
            self.ping_shown();
        }
    }

    /// Pings the app on the screen, unless a ping is in flight: when that
    /// one comes back, the app is pinged again.
    fn ping_shown(&mut self) {
        let reading = &mut self.reading;
        if reading.ping.as_ref().is_some_and(|(app, _)| app.alive()) {
            return;
        }
        reading.ping = None;
        let app = self
            .shown
            .as_ref()
            .and_then(|w| w.toplevel())
            .map(|t| t.client());
        match app {
            Some(app) if app.send_ping(SERIAL_COUNTER.next_serial()).is_ok() => {
                reading.ping = Some((app, reading.given));
            }
            // No app, or one that is gone: nothing is left to read the input.
            _ => reading.read = reading.given,
        }
    }
}
