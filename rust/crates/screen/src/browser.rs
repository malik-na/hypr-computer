//! The browser on a screen: a Chromium-family browser, started the first
//! time the agent opens a page, as an app on the screen like any other (so it
//! ends with the screen), with a fresh profile in the screen's own directory
//! and a home of its own (so both are deleted with it, and none of the user's
//! browser settings, certificates or caches are used), extensions off and
//! accessibility on.
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

use domain::limits::BROWSER;

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
/// Where the user's own files and settings are, which the browser's home
/// stands in for.
const USERS_DIRS: &[&str] = &[
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
];
/// How long a page may take to load before `open_url` stops waiting.
const LOAD_TIME: Duration = Duration::from_secs(20);
/// How long a browser that is slow to start may take to show its first
/// window, on top of the wait for any app's.
const FIRST_WINDOW_TIME: Duration = Duration::from_secs(30);
/// How long a loaded page may take to paint two frames.
const PAINT_TIME: Duration = Duration::from_secs(2);
/// The page the browser starts on. Not `about:blank`: on that, Chromium puts
/// the keyboard in the address bar, and it stays there through `open_url`,
/// so the agent's typing would go there and not to the page.
const START_PAGE: &str = "data:,";
/// Resolves once the page has painted, and then painted again.
const TWO_FRAMES: &str =
    "new Promise(done => requestAnimationFrame(() => requestAnimationFrame(() => done())))";

/// A running browser, and the tabs the agent's pages open in.
pub(crate) struct Browser {
    devtools: DevTools,
    /// The tab normal pages open in.
    tab: Option<Tab>,
    /// The tab private pages open in, in a window of its own.
    private_tab: Option<Tab>,
    /// The private windows' browser context: off the record, with cookies of
    /// its own, all gone with the browser.
    private: Option<String>,
    /// How many windows the screen had when the browser started, until its
    /// first window is on the screen.
    no_window_yet: Option<u64>,
}

/// A page target, and the DevTools session attached to it.
#[derive(Clone)]
struct Tab {
    target: String,
    session: String,
}

/// A page target, as `Target.getTargets` lists it.
struct Page {
    target: String,
    context: String,
    url: String,
}

impl ScreenChild {
    pub(crate) async fn open_web_page(&self, page: WebPage) -> Result<OpenedPage, ScreenError> {
        let mut running = self.browser.lock().await;
        let (tab, on_start_page) = match self.tab_in(&mut running, page.private).await {
            // It exited as this call began, as when the agent has just
            // closed its last window: start it again.
            Err(_) if running.as_ref().is_some_and(|b| b.devtools.closed()) => {
                self.tab_in(&mut running, page.private).await?
            }
            other => other?,
        };
        let Some(browser) = running.as_mut() else {
            return Err(ScreenError("the browser has exited".to_owned()));
        };
        let opened = browser.navigate(&tab, &page.url).await?;
        if on_start_page {
            // The history holds only pages the agent opened: Back can't
            // lead to the start page.
            browser
                .devtools
                .call("Page.resetNavigationHistory", json!({}), Some(&tab.session))
                .await?;
        }
        if let Some(before) = browser.no_window_yet.take() {
            // A browser's first start from a cold disk can take half a
            // minute to show its window; the page is on the screen only then.
            let start = Instant::now();
            while self.status().await?.windows <= before && start.elapsed() < FIRST_WINDOW_TIME {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        self.settled().await?;
        Ok(opened)
    }

    /// The tab `private` or normal pages open in, in the running browser,
    /// which is started first if it isn't running; and whether it shows the
    /// start page.
    async fn tab_in(
        &self,
        running: &mut Option<Browser>,
        private: bool,
    ) -> Result<(Tab, bool), ScreenError> {
        if running.as_ref().is_some_and(|b| b.devtools.closed()) {
            *running = None;
        }
        let browser = match running {
            Some(browser) => browser,
            None => running.insert(self.start_browser().await?),
        };
        browser.tab(private).await
    }

    async fn start_browser(&self) -> Result<Browser, ScreenError> {
        let program = self.browser_program()?;
        let shown = program.to_string_lossy().into_owned();
        let profile = self.dir.join("browser");
        // A home of its own too: Chromium keeps some things there whatever
        // its profile, like the certificates in ~/.pki.
        let home = self.dir.join("browser-home");
        for dir in [&profile, &home] {
            match DirBuilder::new().mode(0o700).create(dir) {
                Err(e) if e.kind() != io::ErrorKind::AlreadyExists => {
                    return Err(ScreenError(format!(
                        "can't make the browser's {}: {e}",
                        dir.display()
                    )));
                }
                _ => {}
            }
        }
        let mut profile_arg = OsString::from("--user-data-dir=");
        profile_arg.push(&profile);
        let (devtools, ends) = DevTools::open()?;
        let mut command = self.app_command(&program, &[])?;
        command.env("HOME", &home);
        for name in USERS_DIRS {
            command.env_remove(name);
        }
        command
            .arg(profile_arg)
            .args([
                "--remote-debugging-pipe",
                // No extension opens pages of its own, even one the system's
                // browser command loads everywhere.
                "--disable-extensions",
                // Pages show to accessibility tools.
                "--force-renderer-accessibility",
                "--ozone-platform=wayland",
                "--no-first-run",
                "--no-default-browser-check",
                "--password-store=basic",
            ])
            // Every request through the daemon's proxy, which keeps to the
            // user's site limits: pages on this machine too, and nothing on
            // QUIC or WebRTC's UDP, which would go around it.
            .arg(format!("--proxy-server=http://{}", self.proxy))
            .args([
                "--proxy-bypass-list=<-loopback>",
                "--disable-quic",
                "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
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
        let before = self.status().await?.windows;
        match self.run_app(command, BROWSER).await {
            Ok(launched @ (Launched::Window { .. } | Launched::NoWindow { .. })) => Ok(Browser {
                devtools,
                tab: None,
                private_tab: None,
                private: None,
                no_window_yet: matches!(launched, Launched::NoWindow { .. }).then_some(before),
            }),
            Ok(Launched::PasswordManager) => Err(ScreenError(format!(
                "the browser {shown}'s window was refused as a password manager's"
            ))),
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
    /// The tab `private` or normal pages open in: the last one, if it is
    /// still there, or else another of the browser's normal pages, or else a
    /// new window. And whether it shows the start page.
    async fn tab(&mut self, private: bool) -> Result<(Tab, bool), ScreenError> {
        let context = match (private, &self.private) {
            (false, _) => None,
            (true, Some(context)) => Some(context.clone()),
            (true, None) => {
                let created = self
                    .devtools
                    .call("Target.createBrowserContext", json!({}), None)
                    .await?;
                let context = string(&created, "browserContextId")?;
                self.private = Some(context.clone());
                Some(context)
            }
        };
        let pages: Vec<Page> = self
            .pages()
            .await?
            .into_iter()
            .filter(|p| match &context {
                Some(private) => p.context == *private,
                None => self.private.as_ref() != Some(&p.context),
            })
            .collect();
        let last = if private {
            &self.private_tab
        } else {
            &self.tab
        };
        let found = match last {
            Some(tab) => pages.iter().find(|p| p.target == tab.target),
            None => None,
        };
        if let (Some(tab), Some(page)) = (last, found) {
            return Ok((tab.clone(), page.url == START_PAGE));
        }
        let target = match (pages.first(), &context) {
            (Some(page), None) => page.target.clone(),
            _ => {
                let mut params = json!({"url": START_PAGE, "newWindow": true});
                if let Some(context) = &context {
                    params["browserContextId"] = json!(context);
                }
                let created = self
                    .devtools
                    .call("Target.createTarget", params, None)
                    .await?;
                string(&created, "targetId")?
            }
        };
        let on_start_page = pages
            .iter()
            .find(|p| p.target == target)
            .is_none_or(|p| p.url == START_PAGE);
        let tab = self.attach(target).await?;
        *(if private {
            &mut self.private_tab
        } else {
            &mut self.tab
        }) = Some(tab.clone());
        Ok((tab, on_start_page))
    }

    /// The browser's pages, in every window.
    async fn pages(&self) -> Result<Vec<Page>, ScreenError> {
        let targets = self
            .devtools
            .call("Target.getTargets", json!({}), None)
            .await?;
        let field = |t: &Value, name: &str| t[name].as_str().unwrap_or_default().to_owned();
        Ok(targets["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["type"] == "page")
            .map(|t| Page {
                target: field(t, "targetId"),
                context: field(t, "browserContextId"),
                url: field(t, "url"),
            })
            .collect())
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
        // Its tab in front, and its window in front of the others.
        self.devtools
            .call("Page.bringToFront", json!({}), session)
            .await?;
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
