//! Password managers and password prompts are never shown on the screen,
//! whoever starts them (specs/Limits.tla, `PasswordManagerNeverShown`). A
//! window is vetted before it is first configured, so before it can draw,
//! and again whenever its app id changes: by its app id, and by the program
//! of the process that made it (`domain::limits::is_password_manager`). One
//! that is a password manager's is never mapped, its connection is closed
//! and its process is killed.
//!
//! Vetting, mapping and drawing all happen on the one event loop, so no
//! screenshot can be taken between a window appearing and being vetted:
//! v0.8 hid such windows only if they were there when a capture began
//! (fde9289), and one that appeared during it was in the picture.

use std::fs;
use std::path::Path;

use domain::limits::is_password_manager;
use rustix::process::{Pid, Signal, kill_process};
use screen::start_time;
use smithay::reexports::wayland_server::backend::DisconnectReason;
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgToplevelSurfaceData};

use crate::state::{ClientState, State};

impl State {
    /// Notes which process `client` is, as it connects: its pid, and when it
    /// started, so a later look at the pid can tell if it is still that
    /// process.
    pub fn note_process(&self, client: &Client) {
        let Ok(credentials) = client.get_credentials(&self.display) else {
            return;
        };
        if let Some(data) = client.get_data::<ClientState>()
            && let Some(since) = u32::try_from(credentials.pid).ok().and_then(start_time)
        {
            let _ = data.process.set((credentials.pid, since));
        }
    }

    /// Shows `toplevel`, a new window making its first commit, unless it is
    /// a password manager's.
    pub fn vet_new_window(&mut self, toplevel: ToplevelSurface) {
        match self.password_manager(&toplevel) {
            Some(name) => self.refuse(&toplevel, &name),
            None => {
                self.add_window(toplevel.clone());
                toplevel.send_configure();
            }
        }
    }

    /// `toplevel` has a new app id: a password manager's is taken off the
    /// screen before anything draws again.
    pub fn vet_app_id(&mut self, toplevel: &ToplevelSurface) {
        if let Some(id) = app_id(toplevel).filter(|id| is_password_manager(id)) {
            self.refuse(toplevel, &id);
        }
    }

    /// What says `toplevel` is a password manager's window, if anything
    /// does: its app id, or the name of the program that made it.
    fn password_manager(&self, toplevel: &ToplevelSurface) -> Option<String> {
        if let Some(id) = app_id(toplevel).filter(|id| is_password_manager(id)) {
            return Some(id);
        }
        let pid = self.pid_of(toplevel)?;
        program_names(pid)
            .into_iter()
            .find(|name| is_password_manager(name))
    }

    /// The process that made `toplevel`, if it is still the one that
    /// connected: a pid the process left behind may be another's by now.
    fn pid_of(&self, toplevel: &ToplevelSurface) -> Option<i32> {
        let client = toplevel.wl_surface().client()?;
        let (pid, since) = *client.get_data::<ClientState>()?.process.get()?;
        let now = u32::try_from(pid).ok().and_then(start_time)?;
        (now == since).then_some(pid)
    }

    /// Takes `toplevel` off the screen for good, closes its app's
    /// connection and kills its process.
    fn refuse(&mut self, toplevel: &ToplevelSurface, name: &str) {
        eprintln!(
            "hypr-computer-screen: ended {name}: password managers and password prompts are \
             never shown to agents"
        );
        self.pending.retain(|t| t != toplevel);
        self.remove_window(toplevel);
        let pid = self.pid_of(toplevel);
        if let Some(client) = toplevel.wl_surface().client() {
            self.display
                .backend_handle()
                .kill_client(client.id(), DisconnectReason::ConnectionClosed);
        }
        let own = i32::try_from(std::process::id()).ok();
        if let Some(pid) = pid.filter(|pid| *pid > 1 && Some(*pid) != own)
            && let Some(pid) = Pid::from_raw(pid)
        {
            let _ = kill_process(pid, Signal::KILL);
        }
        self.refused += 1;
        self.dirty = true;
    }
}

fn app_id(toplevel: &ToplevelSurface) -> Option<String> {
    with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| data.lock().ok())
            .and_then(|data| data.app_id.clone())
    })
}

/// The names process `pid` goes by: its program's file name, its command
/// name, and the file name it was started as.
fn program_names(pid: i32) -> Vec<String> {
    let proc = Path::new("/proc").join(pid.to_string());
    let file_name = |path: &Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    let mut names = Vec::new();
    if let Ok(exe) = fs::read_link(proc.join("exe")) {
        names.extend(file_name(&exe));
    }
    if let Ok(comm) = fs::read_to_string(proc.join("comm")) {
        names.push(comm.trim_end().to_owned());
    }
    if let Ok(cmdline) = fs::read(proc.join("cmdline"))
        && let Some(first) = cmdline.split(|b| *b == 0).next()
    {
        names.extend(file_name(Path::new(&*String::from_utf8_lossy(first))));
    }
    names
}
