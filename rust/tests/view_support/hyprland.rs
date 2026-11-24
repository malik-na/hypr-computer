//! A fake Hyprland 0.57: its IPC socket and its event socket, at
//! `$XDG_RUNTIME_DIR/hypr/<signature>/.socket.sock` and `.socket2.sock`.
//!
//! It answers as Hyprland does: one request per connection, `flags/command`
//! (`j/clients`, `/dispatch hl.dsp.focus({workspace = 10})`,
//! `/eval hl.window_rule({...})`), with JSON, `ok` or an error. It keeps a
//! small model of the user's desktop, as the Python suite's fake does:
//!
//! - One monitor, showing workspace 1, where the user's own window has focus.
//! - A window that opens (reported by the host compositor) lands where the
//!   window rules matching its class and title, or the exec rules of the
//!   process that started it, put it. Without such a rule it opens on the
//!   shown workspace and takes focus. A rule's workspace without " silent"
//!   shows that workspace and focuses the window; with " silent" nothing
//!   changes for the user.
//! - A window asking to be activated (xdg-activation) gets focus, its
//!   workspace shown, as Omarchy's `misc:focus_on_activate` does, unless a
//!   rule says `focus_on_activate = false` or `no_focus = true`.
//! - `exec_cmd` starts the command as Hyprland does, with Hyprland's
//!   environment.
//!
//! Every request, every change of the shown workspace or the focus, and every
//! notification is recorded for the tests.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::lua::{self, Lua};
use super::regex;

/// The workspace the user is on.
pub const USER_WORKSPACE: &str = "1";
/// The user's own window, focused when a test starts.
pub const USER_WINDOW: &str = "0x5e1f";

/// A window as Hyprland lists it.
#[derive(Clone, Debug)]
pub struct Window {
    pub address: String,
    pub pid: i32,
    pub class: String,
    pub title: String,
    pub initial_class: String,
    pub initial_title: String,
    pub workspace: String,
    pub floating: bool,
    focus_order: u64,
    effects: Effects,
}

/// What the rules matching a window do to it.
#[derive(Clone, Debug, Default)]
struct Effects {
    workspace: Option<(String, bool)>,
    float: Option<bool>,
    no_initial_focus: bool,
    no_focus: bool,
    focus_on_activate: Option<bool>,
}

/// Everything the fake knows and has seen.
#[derive(Debug, Default)]
pub struct World {
    /// Every request, in order, with when it came.
    pub requests: Vec<(Instant, String)>,
    /// Requests the fake didn't understand (answered `ok` or an error).
    pub not_understood: Vec<String>,
    /// Every event sent on the event socket.
    pub events: Vec<String>,
    pub windows: Vec<Window>,
    pub shown_workspace: String,
    pub focused: Option<String>,
    /// Each workspace the user was moved to, in order.
    pub workspace_changes: Vec<String>,
    /// Each window focus moved to, in order (`None`: no window).
    pub focus_changes: Vec<Option<String>>,
    /// Each window that asked to be activated.
    pub activation_requests: Vec<String>,
    /// The text of every notification shown through Hyprland.
    pub notifications: Vec<String>,
    pub window_rules: Vec<Lua>,
    exec_rules: Vec<(u32, Lua)>,
    children: Vec<Child>,
    readers: Vec<UnixStream>,
    next_address: u64,
    focus_counter: u64,
    spawn_env: Vec<(String, OsString)>,
    log_dir: PathBuf,
}

pub struct FakeHyprland {
    world: Arc<Mutex<World>>,
    instance: PathBuf,
    stop: Arc<AtomicBool>,
    user_app: Child,
}

impl FakeHyprland {
    /// Listens at `<runtime_dir>/hypr/<signature>/`. Commands it runs get
    /// `spawn_env` as their whole environment.
    pub fn start(
        runtime_dir: &Path,
        signature: &str,
        spawn_env: Vec<(String, OsString)>,
        log_dir: &Path,
    ) -> Self {
        let instance = runtime_dir.join("hypr").join(signature);
        fs::create_dir_all(&instance).unwrap();
        // The user's own app: a real process, so its pid is one.
        let user_app = Command::new("sleep").arg("600").spawn().unwrap();
        let mut world = World {
            shown_workspace: USER_WORKSPACE.to_owned(),
            focused: Some(USER_WINDOW.to_owned()),
            next_address: 0x5e1f_0000,
            spawn_env,
            log_dir: log_dir.to_owned(),
            ..World::default()
        };
        world.windows.push(Window {
            address: USER_WINDOW.to_owned(),
            pid: user_app.id() as i32,
            class: "org.user.Terminal".to_owned(),
            title: "the user's work".to_owned(),
            initial_class: "org.user.Terminal".to_owned(),
            initial_title: "the user's work".to_owned(),
            workspace: USER_WORKSPACE.to_owned(),
            floating: false,
            focus_order: 0,
            effects: Effects::default(),
        });
        let world = Arc::new(Mutex::new(world));
        let stop = Arc::new(AtomicBool::new(false));

        let requests = UnixListener::bind(instance.join(".socket.sock")).unwrap();
        let (w, s) = (world.clone(), stop.clone());
        thread::spawn(move || {
            for conn in requests.incoming() {
                if s.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(conn) = conn {
                    serve(&w, conn);
                }
            }
        });
        let events = UnixListener::bind(instance.join(".socket2.sock")).unwrap();
        let (w, s) = (world.clone(), stop.clone());
        thread::spawn(move || {
            for conn in events.incoming() {
                if s.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(conn) = conn {
                    w.lock().unwrap().readers.push(conn);
                }
            }
        });
        FakeHyprland {
            world,
            instance,
            stop,
            user_app,
        }
    }

    pub fn world(&self) -> MutexGuard<'_, World> {
        self.world.lock().unwrap()
    }

    pub fn shared(&self) -> Arc<Mutex<World>> {
        self.world.clone()
    }

    /// The user switches workspace, as with Super+<n>.
    pub fn user_shows_workspace(&self, name: &str) {
        self.world().show_workspace(name);
    }

    /// Everything the fake saw, for a failing test's message.
    pub fn report(&self) -> String {
        let w = self.world();
        let mut out = String::from("fake Hyprland:\n  requests:\n");
        for (_, r) in &w.requests {
            out += &format!("    {r:?}\n");
        }
        out += &format!("  not understood: {:?}\n", w.not_understood);
        out += &format!("  events: {:?}\n", w.events);
        out += &format!(
            "  shown workspace {:?}, focused {:?}\n  workspace changes {:?}\n  focus changes {:?}\n  activation requests {:?}\n  notifications {:?}\n",
            w.shown_workspace,
            w.focused,
            w.workspace_changes,
            w.focus_changes,
            w.activation_requests,
            w.notifications
        );
        for win in &w.windows {
            out += &format!(
                "  window {} pid {} class {:?} title {:?} on {:?} floating {}\n",
                win.address, win.pid, win.class, win.title, win.workspace, win.floating
            );
        }
        out
    }
}

impl Drop for FakeHyprland {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for name in [".socket.sock", ".socket2.sock"] {
            let _ = UnixStream::connect(self.instance.join(name));
        }
        let _ = self.user_app.kill();
        let _ = self.user_app.wait();
        let mut w = self.world();
        for child in &mut w.children {
            // Its whole process group, as the command may have forked.
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .stderr(Stdio::null())
                .status();
            let _ = child.kill();
            let _ = child.wait();
        }
        w.readers.clear();
    }
}

fn serve(world: &Arc<Mutex<World>>, mut conn: UnixStream) {
    conn.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut buf = vec![0; 1 << 20];
    let mut len = conn.read(&mut buf).unwrap_or(0);
    // A long request may come in more than one piece.
    conn.set_read_timeout(Some(Duration::from_millis(5)))
        .unwrap();
    while len < buf.len() {
        match conn.read(&mut buf[len..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => len += n,
        }
    }
    let request = String::from_utf8_lossy(&buf[..len]).into_owned();
    let reply = world.lock().unwrap().handle(&request);
    let _ = conn.write_all(reply.as_bytes());
    let _ = conn.shutdown(std::net::Shutdown::Both);
}

impl World {
    fn handle(&mut self, request: &str) -> String {
        self.requests.push((Instant::now(), request.to_owned()));
        if let Some(batch) = request.strip_prefix("[[BATCH]]") {
            return batch
                .split(';')
                .map(|r| self.command(r.trim()))
                .collect::<Vec<_>>()
                .join("\n\n");
        }
        self.command(request)
    }

    /// One `flags/command args` request.
    fn command(&mut self, request: &str) -> String {
        let (flags, command) = match request.split_once('/') {
            Some((flags, rest)) if flags.chars().all(|c| c.is_ascii_alphabetic()) => (flags, rest),
            _ => ("", request),
        };
        let json = flags.contains('j');
        let (verb, args) = command
            .split_once(char::is_whitespace)
            .unwrap_or((command, ""));
        let args = args.trim();
        match verb {
            "clients" if json => self.clients_json().to_string(),
            "activewindow" if json => self.active_window_json().to_string(),
            "activeworkspace" if json => self.workspace_json(&self.shown_workspace).to_string(),
            "workspaces" if json => Value::Array(
                self.workspace_names()
                    .iter()
                    .map(|n| self.workspace_json(n))
                    .collect(),
            )
            .to_string(),
            "monitors" if json => json!([self.monitor_json()]).to_string(),
            "version" if json => {
                json!({"branch": "", "commit": "", "version": "0.57.2", "tag": "v0.57.2", "flags": []}).to_string()
            }
            "activeworkspace" => format!(
                "workspace ID {0} ({0}) on monitor FAKE-1:\n",
                self.shown_workspace
            ),
            "version" => "Hyprland 0.57.2 built from branch  at commit  (fake).\nTag: v0.57.2\n".to_owned(),
            "dispatch" => self.dispatch(args),
            "eval" => self.eval(args),
            "notify" => {
                self.notifications.push(args.to_owned());
                "ok".to_owned()
            }
            "keyword" | "setprop" | "dismissnotify" => {
                self.not_understood.push(request.to_owned());
                "ok".to_owned()
            }
            _ => {
                self.not_understood.push(request.to_owned());
                format!("error: unknown request {request:?}")
            }
        }
    }

    fn eval(&mut self, code: &str) -> String {
        let mut understood = false;
        for args in lua::calls(code, "hl.window_rule") {
            if let Some(rule @ Lua::Table(_)) = args.into_iter().next() {
                self.window_rules.push(rule);
                understood = true;
            }
        }
        for args in lua::calls(code, "hl.notification.create") {
            let text = args
                .first()
                .and_then(|a| a.get("text").or(a.get("message")).and_then(Lua::text))
                .unwrap_or_else(|| format!("{args:?}"));
            self.notifications.push(text);
            understood = true;
        }
        // `hl.dispatch(hl.dsp.focus({...}))` and the like.
        let mut from = 0;
        while let Some(at) = code[from..].find("hl.dsp.") {
            let start = from + at;
            let end = code[start..]
                .find('(')
                .map(|p| start + p)
                .unwrap_or(code.len());
            let name = &code[start + "hl.dsp.".len()..end];
            if name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
                && end < code.len()
            {
                self.dispatch(&code[start..]);
                understood = true;
            }
            from = end.max(start + 1);
        }
        if !understood {
            self.not_understood.push(format!("eval {code}"));
        }
        "ok".to_owned()
    }

    fn dispatch(&mut self, args: &str) -> String {
        if let Some(call) = args.strip_prefix("hl.dsp.") {
            let Some(paren) = call.find('(') else {
                self.not_understood.push(format!("dispatch {args}"));
                return "error: not a call".to_owned();
            };
            let name = &call[..paren];
            let Some(values) = lua::arguments(&call[paren + 1..]) else {
                self.not_understood.push(format!("dispatch {args}"));
                return "error: can't read the arguments".to_owned();
            };
            let first = values.first().cloned().unwrap_or(Lua::Nil);
            match name {
                "exec_cmd" => {
                    let cmd = first.text().unwrap_or_default();
                    let rules = values.get(1).cloned().unwrap_or(Lua::Table(Vec::new()));
                    self.exec(&cmd, rules);
                }
                "focus" => {
                    if let Some(ws) = first.get("workspace").and_then(Lua::text) {
                        self.show_workspace(&workspace_name(&ws));
                    } else if let Some(window) = first.get("window").and_then(Lua::text) {
                        self.focus_window(&window);
                    } else {
                        self.not_understood.push(format!("dispatch {args}"));
                    }
                }
                "window.move" => {
                    let ws = first.get("workspace").and_then(Lua::text);
                    let window = first
                        .get("window")
                        .and_then(Lua::text)
                        .or_else(|| self.focused.clone());
                    let follow = first.get("follow").and_then(Lua::as_bool) == Some(true);
                    if let (Some(ws), Some(window)) = (ws, window) {
                        self.move_window(&window, &ws, follow);
                    }
                }
                _ => self.not_understood.push(format!("dispatch {args}")),
            }
            return "ok".to_owned();
        }
        // The dispatchers of Hyprland before Lua, in case they're used.
        let (name, rest) = args.split_once(' ').unwrap_or((args, ""));
        match name {
            "workspace" => self.show_workspace(&workspace_name(rest.trim())),
            "focuswindow" => self.focus_window(rest.trim()),
            "movetoworkspace" | "movetoworkspacesilent" => {
                let (ws, window) = rest.split_once(',').unwrap_or((rest, ""));
                let window = if window.is_empty() {
                    self.focused.clone().unwrap_or_default()
                } else {
                    window.to_owned()
                };
                self.move_window(&window, ws.trim(), name == "movetoworkspace");
            }
            "exec" => {
                let rest = rest.trim();
                if let Some(rules) = rest.strip_prefix('[') {
                    let (rules, cmd) = rules.split_once(']').unwrap_or((rules, ""));
                    let mut fields = Vec::new();
                    for rule in rules.split(';') {
                        let rule = rule.trim();
                        if let Some(ws) = rule.strip_prefix("workspace ") {
                            fields.push((Some("workspace".to_owned()), Lua::Str(ws.to_owned())));
                        } else if rule == "float" {
                            fields.push((Some("float".to_owned()), Lua::Bool(true)));
                        } else if rule == "noinitialfocus" {
                            fields.push((Some("no_initial_focus".to_owned()), Lua::Bool(true)));
                        }
                    }
                    self.exec(cmd.trim(), Lua::Table(fields));
                } else {
                    self.exec(rest, Lua::Table(Vec::new()));
                }
            }
            _ => self.not_understood.push(format!("dispatch {args}")),
        }
        "ok".to_owned()
    }

    fn exec(&mut self, cmd: &str, rules: Lua) {
        let n = self.children.len();
        let out = fs::File::create(self.log_dir.join(format!("exec-{n}.log"))).unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", cmd])
            .env_clear()
            .envs(self.spawn_env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .process_group(0);
        match command.spawn() {
            Ok(child) => {
                self.exec_rules.push((child.id(), rules));
                self.children.push(child);
            }
            Err(e) => self.not_understood.push(format!("exec {cmd}: {e}")),
        }
    }

    fn emit(&mut self, event: String) {
        let line = format!("{event}\n");
        self.readers
            .retain_mut(|r| r.write_all(line.as_bytes()).is_ok());
        self.events.push(event);
    }

    fn window(&self, address: &str) -> Option<&Window> {
        let address = address.strip_prefix("address:").unwrap_or(address);
        self.windows.iter().find(|w| w.address == address)
    }

    fn window_mut(&mut self, address: &str) -> Option<&mut Window> {
        let address = address.strip_prefix("address:").unwrap_or(address);
        self.windows.iter_mut().find(|w| w.address == address)
    }

    /// The workspace the window at `address` is on.
    pub fn workspace_of(&self, address: &str) -> Option<String> {
        self.window(address).map(|w| w.workspace.clone())
    }

    pub fn find(&self, address: &str) -> Option<Window> {
        self.window(address).cloned()
    }

    /// Whether the window at `address` is on the shown workspace.
    pub fn is_visible(&self, address: &str) -> bool {
        self.window(address)
            .is_some_and(|w| w.workspace == self.shown_workspace)
    }

    /// Shows workspace `name` and focuses its most recently focused window.
    pub fn show_workspace(&mut self, name: &str) {
        if self.shown_workspace == name {
            return;
        }
        self.shown_workspace = name.to_owned();
        self.workspace_changes.push(name.to_owned());
        self.emit(format!("workspace>>{name}"));
        self.emit(format!("workspacev2>>{},{name}", workspace_id(name)));
        let last = self
            .windows
            .iter()
            .filter(|w| w.workspace == name)
            .max_by_key(|w| w.focus_order)
            .map(|w| w.address.clone());
        self.set_focus(last);
    }

    fn set_focus(&mut self, address: Option<String>) {
        if self.focused == address {
            return;
        }
        self.focused = address.clone();
        self.focus_changes.push(address.clone());
        self.focus_counter += 1;
        let counter = self.focus_counter;
        let (class, title) = match address.as_deref().and_then(|a| self.window_mut(a)) {
            Some(w) => {
                w.focus_order = counter;
                (w.class.clone(), w.title.clone())
            }
            None => (String::new(), String::new()),
        };
        self.emit(format!("activewindow>>{class},{title}"));
        let bare = address.as_deref().map(bare_address).unwrap_or_default();
        self.emit(format!("activewindowv2>>{bare}"));
    }

    fn focus_window(&mut self, address: &str) {
        let Some(w) = self.window(address) else {
            self.not_understood
                .push(format!("focus {address}: no such window"));
            return;
        };
        let (ws, address) = (w.workspace.clone(), w.address.clone());
        self.show_workspace(&ws);
        self.set_focus(Some(address));
    }

    fn move_window(&mut self, address: &str, ws: &str, follow: bool) {
        let ws = workspace_name(ws.trim_end_matches(" silent"));
        let Some(w) = self.window_mut(address) else {
            return;
        };
        w.workspace = ws.clone();
        let bare = bare_address(&w.address);
        let address = w.address.clone();
        self.emit(format!("movewindow>>{bare},{ws}"));
        if follow {
            self.show_workspace(&ws);
            self.set_focus(Some(address));
        } else if self.focused.as_deref() == Some(address.as_str()) {
            let shown = self.shown_workspace.clone();
            let next = self
                .windows
                .iter()
                .filter(|w| w.workspace == shown)
                .max_by_key(|w| w.focus_order)
                .map(|w| w.address.clone());
            self.set_focus(next);
        }
    }

    /// A window maps: the rules decide where it goes and whether it takes
    /// focus. Returns its address.
    pub fn open_window(&mut self, pid: i32, class: &str, title: &str) -> String {
        let mut fx = Effects::default();
        let rules: Vec<Lua> = self
            .window_rules
            .iter()
            .filter(|r| r.get("enabled").and_then(Lua::as_bool) != Some(false))
            .filter(|r| self.rule_matches(r, class, title))
            .cloned()
            .collect();
        for rule in &rules {
            apply(&mut fx, rule);
        }
        let ancestors = ancestors(pid);
        let exec: Vec<Lua> = self
            .exec_rules
            .iter()
            .filter(|(p, _)| ancestors.contains(p))
            .map(|(_, r)| r.clone())
            .collect();
        for rule in &exec {
            apply(&mut fx, rule);
        }
        self.next_address += 1;
        let address = format!("0x{:x}", self.next_address);
        let (ws, silent) = fx
            .workspace
            .clone()
            .unwrap_or((self.shown_workspace.clone(), false));
        self.windows.push(Window {
            address: address.clone(),
            pid,
            class: class.to_owned(),
            title: title.to_owned(),
            initial_class: class.to_owned(),
            initial_title: title.to_owned(),
            workspace: ws.clone(),
            floating: fx.float.unwrap_or(false),
            focus_order: 0,
            effects: fx.clone(),
        });
        self.emit(format!(
            "openwindow>>{},{ws},{class},{title}",
            bare_address(&address)
        ));
        let takes_focus = !(fx.no_initial_focus || fx.no_focus);
        if ws != self.shown_workspace {
            if !silent {
                self.show_workspace(&ws);
                if takes_focus {
                    self.set_focus(Some(address.clone()));
                }
            }
        } else if takes_focus {
            self.set_focus(Some(address.clone()));
        }
        address
    }

    pub fn retitle(&mut self, address: &str, title: &str) {
        if let Some(w) = self.window_mut(address) {
            w.title = title.to_owned();
            let bare = bare_address(address);
            self.emit(format!("windowtitle>>{bare}"));
            self.emit(format!("windowtitlev2>>{bare},{title}"));
        }
    }

    pub fn close_window(&mut self, address: &str) {
        let Some(index) = self.windows.iter().position(|w| w.address == address) else {
            return;
        };
        self.windows.remove(index);
        self.emit(format!("closewindow>>{}", bare_address(address)));
        if self.focused.as_deref() == Some(address) {
            let shown = self.shown_workspace.clone();
            let next = self
                .windows
                .iter()
                .filter(|w| w.workspace == shown)
                .max_by_key(|w| w.focus_order)
                .map(|w| w.address.clone());
            self.set_focus(next);
        }
    }

    /// A window asks to be activated (xdg-activation).
    pub fn activate(&mut self, address: &str) {
        self.activation_requests.push(address.to_owned());
        let Some(w) = self.window(address) else {
            return;
        };
        if w.effects.no_focus || w.effects.focus_on_activate == Some(false) {
            return;
        }
        let address = w.address.clone();
        self.focus_window(&address);
    }

    fn rule_matches(&self, rule: &Lua, class: &str, title: &str) -> bool {
        let Some(m) = rule.get("match") else {
            return false;
        };
        m.fields().iter().all(|(key, value)| {
            let pattern = value.text().unwrap_or_default();
            let text = match key.as_deref() {
                Some("class" | "initial_class") => class,
                Some("title" | "initial_title") => title,
                Some("xwayland") => return value.as_bool() == Some(false),
                Some("float") => return value.as_bool() == Some(false),
                _ => return false,
            };
            regex::full_match(&pattern, text).unwrap_or(false)
        })
    }

    fn workspace_names(&self) -> Vec<String> {
        let mut names: Vec<String> = vec![self.shown_workspace.clone()];
        for w in &self.windows {
            if !names.contains(&w.workspace) {
                names.push(w.workspace.clone());
            }
        }
        names
    }

    fn client_json(&self, w: &Window) -> Value {
        json!({
            "address": w.address, "mapped": true, "hidden": false,
            "at": [0, 0], "size": [1280, 800],
            "workspace": {"id": workspace_id(&w.workspace), "name": w.workspace},
            "floating": w.floating, "pseudo": false, "monitor": 0,
            "class": w.class, "title": w.title,
            "initialClass": w.initial_class, "initialTitle": w.initial_title,
            "pid": w.pid, "xwayland": false, "pinned": false, "fullscreen": 0,
            "fullscreenClient": 0, "grouped": [], "tags": [], "swallowing": "0x0",
            "focusHistoryID": self.focus_history_id(w), "inhibitingIdle": false,
        })
    }

    fn focus_history_id(&self, w: &Window) -> usize {
        let mut orders: Vec<u64> = self.windows.iter().map(|w| w.focus_order).collect();
        orders.sort_unstable_by(|a, b| b.cmp(a));
        orders.iter().position(|&o| o == w.focus_order).unwrap_or(0)
    }

    fn clients_json(&self) -> Value {
        Value::Array(self.windows.iter().map(|w| self.client_json(w)).collect())
    }

    fn active_window_json(&self) -> Value {
        match self.focused.as_deref().and_then(|a| self.window(a)) {
            Some(w) => self.client_json(w),
            None => json!({}),
        }
    }

    fn workspace_json(&self, name: &str) -> Value {
        let on: Vec<&Window> = self
            .windows
            .iter()
            .filter(|w| w.workspace == name)
            .collect();
        let last = on.iter().max_by_key(|w| w.focus_order);
        json!({
            "id": workspace_id(name), "name": name, "monitor": "FAKE-1", "monitorID": 0,
            "windows": on.len(), "hasfullscreen": false,
            "lastwindow": last.map(|w| w.address.clone()).unwrap_or_else(|| "0x0".to_owned()),
            "lastwindowtitle": last.map(|w| w.title.clone()).unwrap_or_default(),
            "ispersistent": false,
        })
    }

    fn monitor_json(&self) -> Value {
        json!({
            "id": 0, "name": "FAKE-1", "description": "fake monitor", "make": "fake", "model": "fake",
            "serial": "", "width": 2560, "height": 1600, "refreshRate": 60.0, "x": 0, "y": 0,
            "activeWorkspace": {"id": workspace_id(&self.shown_workspace), "name": self.shown_workspace},
            "specialWorkspace": {"id": 0, "name": ""},
            "reserved": [0, 0, 0, 0], "scale": 2.0, "transform": 0, "focused": true,
            "dpmsStatus": true, "vrr": false, "disabled": false,
        })
    }
}

fn apply(fx: &mut Effects, rule: &Lua) {
    for (key, value) in rule.fields() {
        match key.as_deref() {
            Some("workspace") => {
                if let Some(text) = value.text() {
                    let silent = text.ends_with(" silent");
                    let name = workspace_name(text.trim_end_matches(" silent"));
                    fx.workspace = Some((name, silent));
                }
            }
            Some("float") => fx.float = value.as_bool(),
            Some("tile") => fx.float = value.as_bool().map(|t| !t),
            Some("no_initial_focus") => fx.no_initial_focus = value.as_bool() == Some(true),
            Some("no_focus") => fx.no_focus = value.as_bool() == Some(true),
            Some("focus_on_activate") => fx.focus_on_activate = value.as_bool(),
            _ => {}
        }
    }
}

fn workspace_name(text: &str) -> String {
    let text = text.trim();
    text.strip_prefix("name:").unwrap_or(text).to_owned()
}

fn workspace_id(name: &str) -> i64 {
    name.parse().unwrap_or(-1)
}

/// Hyprland's events give addresses without the `0x`.
fn bare_address(address: &str) -> String {
    address.trim_start_matches("0x").to_owned()
}

/// `pid` and the pids of its ancestors.
fn ancestors(pid: i32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut pid = pid;
    while pid > 1 && out.len() < 64 {
        out.push(pid as u32);
        let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
            break;
        };
        let Some(after) = stat.rfind(')').map(|i| &stat[i + 1..]) else {
            break;
        };
        pid = after
            .split_whitespace()
            .nth(1)
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
    }
    out
}

/// Windows grouped by workspace, for messages.
pub fn by_workspace(windows: &[Window]) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for w in windows {
        map.entry(w.workspace.clone())
            .or_default()
            .push(w.title.clone());
    }
    map
}
