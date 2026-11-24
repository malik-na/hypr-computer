//! Where open_url lands. The guard checks the page asked for, and the proxy
//! keeps the browser off unlisted sites, but a redirect can still take the
//! page open_url waits for to a site the user didn't list: the proxy's refusal
//! page then shows in its place. The call must fail as a refusal too, not
//! report a page that loaded, so the screen a tool call gets checks where
//! each page it opened ended up.

use std::sync::Arc;
use std::time::SystemTime;

use domain::limits::{Refusal, normalize_site};
use ports::{
    Action, AppCommand, BoxFuture, Launched, OpenedPage, Png, Screen, ScreenError, WebPage,
};

use crate::limits::LimitsFile;

/// A tool call's screen, which refuses a page that landed on a site the
/// limits don't allow, and is otherwise the screen itself.
pub struct Landing {
    screen: Arc<dyn Screen>,
    limits: Arc<LimitsFile>,
}

impl Landing {
    pub fn new(screen: Arc<dyn Screen>, limits: Arc<LimitsFile>) -> Self {
        Self { screen, limits }
    }
}

impl Screen for Landing {
    fn screenshot(&self) -> BoxFuture<'_, Result<Png, ScreenError>> {
        self.screen.screenshot()
    }

    fn act(&self, action: Action) -> BoxFuture<'_, Result<(), ScreenError>> {
        self.screen.act(action)
    }

    fn launch(&self, app: AppCommand) -> BoxFuture<'_, Result<Launched, ScreenError>> {
        self.screen.launch(app)
    }

    fn open_page(&self, page: WebPage) -> BoxFuture<'_, Result<OpenedPage, ScreenError>> {
        Box::pin(async move {
            let opened = self.screen.open_page(page).await?;
            let web = ["http://", "https://"].iter().any(|scheme| {
                opened
                    .url
                    .get(..scheme.len())
                    .is_some_and(|s| s.eq_ignore_ascii_case(scheme))
            });
            if web {
                let host = normalize_site(&opened.url).unwrap_or_default();
                let now = SystemTime::now();
                if !self.limits.load(now).allows_site(&host, now) {
                    return Err(ScreenError(Refusal::Site { host }.to_string()));
                }
            }
            Ok(opened)
        })
    }

    fn front_app(&self) -> BoxFuture<'_, Option<String>> {
        self.screen.front_app()
    }

    fn end_call(&self) -> BoxFuture<'_, Option<String>> {
        self.screen.end_call()
    }

    fn note(&self) -> Option<String> {
        self.screen.note()
    }
}
