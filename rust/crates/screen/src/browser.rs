//! The browser on a screen: a Chromium-family browser, started the first
//! time the agent opens a page, as an app on the screen like any other (so it
//! ends with the screen), with a fresh profile in the screen's own directory
//! (so it is deleted with it).
//!
//! The daemon drives it over its DevTools pipe (see `devtools`). A page has
//! loaded when the browser says its `load` lifecycle event fired for that
//! very navigation (matched by its loader id, so a late event of the page
//! before can't count). Then the page is given two frames to paint, and the
//! screen time to show them. The window title is no signal: Chromium changes
//! it about 200 ms after the pixels, and a page has its title long before it
//! has loaded.

use std::ffi::OsString;
use std::fs::DirBuilder;
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;
use std::time::Duration;

use command_fds::{CommandFdExt, FdMapping};
use ports::{Launched, OpenedPage, ScreenError, WebPage};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, timeout, timeout_at};

use crate::devtools::{CLOSED, DevTools, Event};
use crate::process::ScreenChild;

/// The names a Chromium-family browser goes by, looked for on `PATH` when
/// the daemon isn't told which browser to start.
const NAMES: &[&str] = &[
    "chromium",
    "chromium-browser",
    "google-chrome-stable",
    "google-chrome",
];
/// How long a page may take to load before `open_url` stops waiting.
const LOAD_TIME: Duration = Duration::from_secs(20);
/// How long a loaded page may take to paint two frames.
const PAINT_TIME: Duration = Duration::from_secs(2);
/// The page the browser starts on. Not `about:blank`: on that, Chromium puts
/// the keyboard in the address bar, and it stays there through `open_url`,
/// so the agent's typing would go there and not to the page.
const START_PAGE: &str = "data:,";
/// Resolves once the page has painted, and then painted again.
const TWO_FRAMES: &str =
    "new Promise(done => requestAnimationFrame(() => requestAnimationFrame(() => done())))";

/// A running browser, and the tab the agent's pages open in.
pub(crate) struct Browser {
    devtools: DevTools,
    tab: Option<Tab>,
    /// The start page is still in the history.
    start_page: bool,
}

/// A page target, and the DevTools session attached to it.
#[derive(Clone)]
struct Tab {
    target: String,
    session: String,
}

impl ScreenChild {
    pub(crate) async fn open_web_page(&self, page: WebPage) -> Result<OpenedPage, ScreenError> {
        let mut running = self.browser.lock().await;
        let tab = match self.tab_in(&mut running).await {
            // It exited as this call began, as when the agent has just
            // closed its last window: start it again.
            Err(_) if running.as_ref().is_some_and(|b| b.devtools.closed()) => {
                self.tab_in(&mut running).await?
            }
            other => other?,
        };
        let Some(browser) = running.as_mut() else {
            return Err(ScreenError("the browser has exited".to_owned()));
        };
        let opened = browser.navigate(&tab, &page.url).await?;
        if std::mem::take(&mut browser.start_page) {
            // The history holds only pages the agent opened: Back can't
            // lead to the start page.
            browser
                .devtools
                .call("Page.resetNavigationHistory", json!({}), Some(&tab.session))
                .await?;
        }
        self.settled().await?;
        Ok(opened)
    }

    /// The tab pages open in, in the running browser, which is started
    /// first if it isn't running.
    async fn tab_in(&self, running: &mut Option<Browser>) -> Result<Tab, ScreenError> {
        if running.as_ref().is_some_and(|b| b.devtools.closed()) {
            *running = None;
        }
        let browser = match running {
            Some(browser) => browser,
            None => running.insert(self.start_browser().await?),
        };
        browser.tab().await
    }

    async fn start_browser(&self) -> Result<Browser, ScreenError> {
        let program = self.browser_program()?;
        let shown = program.to_string_lossy().into_owned();
        let profile = self.dir.join("browser");
        match DirBuilder::new().mode(0o700).create(&profile) {
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => {
                return Err(ScreenError(format!(
                    "can't make the browser's profile at {}: {e}",
                    profile.display()
                )));
            }
            _ => {}
        }
        let mut profile_arg = OsString::from("--user-data-dir=");
        profile_arg.push(&profile);
        let (devtools, ends) = DevTools::open()?;
        let mut command = self.app_command(&program, &[])?;
        command
            .arg(profile_arg)
            .args([
                "--remote-debugging-pipe",
                "--ozone-platform=wayland",
                "--no-first-run",
                "--no-default-browser-check",
                "--password-store=basic",
                START_PAGE,
            ])
            .fd_mappings(vec![
                FdMapping {
                    parent_fd: ends.commands,
                    child_fd: 3,
                },
                FdMapping {
                    parent_fd: ends.replies,
                    child_fd: 4,
                },
            ])
            .map_err(|e| ScreenError(format!("can't hand the browser its pipe: {e}")))?;
        match self.run_app(command).await {
            Ok(Launched::Window { .. } | Launched::NoWindow { .. }) => Ok(Browser {
                devtools,
                tab: None,
                start_page: true,
            }),
            Ok(Launched::Exited { status, output, .. }) => Err(ScreenError(format!(
                "the browser {shown} exited ({status}) before it opened a window: {output}"
            ))),
            Err(e) => Err(ScreenError(format!("can't start the browser {shown}: {e}"))),
        }
    }

    /// The browser the daemon was told to start, or else the first
    /// Chromium-family browser on `PATH`.
    fn browser_program(&self) -> Result<PathBuf, ScreenError> {
        if let Some(given) = &self.browser_program {
            return Ok(given.clone());
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        NAMES
            .iter()
            .find_map(|name| {
                std::env::split_paths(&path)
                    .filter(|dir| dir.is_absolute())
                    .map(|dir| dir.join(name))
                    .find(|p| {
                        p.metadata()
                            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                    })
            })
            .ok_or_else(|| {
                ScreenError(format!(
                    "no browser is installed: install Chromium, or set HYPR_COMPUTER_BROWSER \
                     to a Chromium-family browser for the daemon (looked for {} on PATH)",
                    NAMES.join(", ")
                ))
            })
    }
}

impl Browser {
    /// The tab pages open in: the last one, if it is still there, or else
    /// the first page the browser has, or else a new one.
    async fn tab(&mut self) -> Result<Tab, ScreenError> {
        let targets = self
            .devtools
            .call("Target.getTargets", json!({}), None)
            .await?;
        let pages: Vec<&str> = targets["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["type"] == "page")
            .filter_map(|t| t["targetId"].as_str())
            .collect();
        if let Some(tab) = &self.tab
            && pages.contains(&tab.target.as_str())
        {
            return Ok(tab.clone());
        }
        let target = match pages.first() {
            Some(target) => (*target).to_owned(),
            None => {
                let created = self
                    .devtools
                    .call("Target.createTarget", json!({"url": START_PAGE}), None)
                    .await?;
                self.start_page = true;
                string(&created, "targetId")?
            }
        };
        let tab = self.attach(target).await?;
        self.tab = Some(tab.clone());
        Ok(tab)
    }

    /// A session on `target` that reports its lifecycle events.
    async fn attach(&self, target: String) -> Result<Tab, ScreenError> {
        let attached = self
            .devtools
            .call(
                "Target.attachToTarget",
                json!({"targetId": target, "flatten": true}),
                None,
            )
            .await?;
        let session = string(&attached, "sessionId")?;
        for (method, params) in [
            ("Page.enable", json!({})),
            ("Page.setLifecycleEventsEnabled", json!({"enabled": true})),
        ] {
            self.devtools.call(method, params, Some(&session)).await?;
        }
        Ok(Tab { target, session })
    }

    /// Opens `url` in `tab`, and waits until it has loaded and painted.
    async fn navigate(&self, tab: &Tab, url: &str) -> Result<OpenedPage, ScreenError> {
        let session = Some(tab.session.as_str());
        let deadline = Instant::now() + LOAD_TIME;
        let mut events = self.devtools.events();
        // Answered once the server has answered: a slow one is still loading.
        let navigated = self
            .devtools
            .call_within(LOAD_TIME, "Page.navigate", json!({"url": url}), session)
            .await?;
        let navigated = navigated.unwrap_or_default();
        if let Some(error) = navigated["errorText"].as_str().filter(|e| !e.is_empty()) {
            return Err(ScreenError(format!(
                "the browser couldn't load it: {error}"
            )));
        }
        let loaded = match navigated["loaderId"].as_str() {
            None if navigated.is_null() => false,
            // A move within the page, which loads nothing.
            None => true,
            Some(loader) => timeout_at(deadline, load(&mut events, &tab.session, loader))
                .await
                .unwrap_or(Ok(false))?,
        };
        let paint = self.devtools.call(
            "Runtime.evaluate",
            json!({"expression": TWO_FRAMES, "awaitPromise": true}),
            session,
        );
        // A page that can't paint now (it isn't shown) still gets its turn.
        let _ = timeout(PAINT_TIME, paint).await;
        let info = self
            .devtools
            .call(
                "Target.getTargetInfo",
                json!({"targetId": tab.target}),
                None,
            )
            .await?;
        Ok(OpenedPage {
            url: info["targetInfo"]["url"].as_str().unwrap_or(url).to_owned(),
            title: info["targetInfo"]["title"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            loaded,
        })
    }
}

/// Waits for the `load` event of the navigation `loader` in `session`, or
/// of the one that replaced it: a page that sends the browser on (with
/// `location.replace`, say) before it has loaded never fires its own.
async fn load(
    events: &mut tokio::sync::broadcast::Receiver<Event>,
    session: &str,
    loader: &str,
) -> Result<bool, ScreenError> {
    let mut loader = loader.to_owned();
    loop {
        match events.recv().await {
            Ok(event) if event.method == CLOSED => {
                return Err(ScreenError("the browser exited".to_owned()));
            }
            Ok(event)
                if event.method == "Page.frameNavigated"
                    && event.session.as_deref() == Some(session)
                    && event.params["frame"]["parentId"].is_null() =>
            {
                if let Some(next) = event.params["frame"]["loaderId"].as_str() {
                    next.clone_into(&mut loader);
                }
            }
            Ok(event)
                if event.method == "Page.lifecycleEvent"
                    && event.session.as_deref() == Some(session)
                    && event.params["name"] == "load"
                    && event.params["loaderId"] == loader =>
            {
                return Ok(true);
            }
            Ok(_) | Err(RecvError::Lagged(_)) => {}
            Err(RecvError::Closed) => return Ok(false),
        }
    }
}

/// A string field of a DevTools result.
fn string(result: &Value, name: &str) -> Result<String, ScreenError> {
    result[name]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| ScreenError(format!("the browser's answer has no {name}: {result}")))
}
