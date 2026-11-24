//! `open_url`: open a web page in the browser on the agent's screen.

use ports::{BoxFuture, Screen, WebPage};
use serde_json::{Map, Value, json};

use crate::registry::{CallError, Tool, ToolOutput, ToolSpec};

pub struct OpenUrl;

impl Tool for OpenUrl {
    fn spec(&self) -> ToolSpec {
        let schema = json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The page: an http:// or https:// URL.",
                },
                "private": {
                    "type": "boolean",
                    "description": "Open it in a private window, without your session's \
                                    cookies. Default false.",
                },
            },
            "required": ["url"],
        });
        ToolSpec {
            name: "open_url",
            description: "Open a web page in the browser on your screen. Returns once the page \
                          has loaded and is in front, so your next screenshot shows it. The \
                          browser starts the first time, with a fresh profile of your session's \
                          own: cookies last until your session ends. With `private`, the page \
                          opens in a private window, which has none of them.",
            input_schema: schema.as_object().cloned().unwrap_or_default(),
        }
    }

    fn call<'a>(
        &'a self,
        screen: &'a dyn Screen,
        args: Map<String, Value>,
    ) -> BoxFuture<'a, Result<ToolOutput, CallError>> {
        Box::pin(async move {
            let url = web_url(&args)?;
            let private = match args.get("private") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(private)) => *private,
                Some(other) => {
                    return Err(CallError::InvalidArguments(format!(
                        "`private` must be true or false, not {other}"
                    )));
                }
            };
            let page = WebPage {
                url: url.clone(),
                private,
            };
            Ok(match screen.open_page(page).await {
                Ok(page) if page.loaded => ToolOutput::text(format!(
                    "Opened {} ({:?}); it has loaded and is on your screen.",
                    page.url, page.title
                )),
                Ok(page) => ToolOutput::text(format!(
                    "Opened {} ({:?}), but it was still loading when I stopped waiting. \
                     Take a screenshot to see how far it got.",
                    page.url, page.title
                )),
                Err(e) => ToolOutput::error(format!("Couldn't open {url}: {e}")),
            })
        })
    }
}

/// The `url` argument, if it is an http or https URL with a host. Other
/// schemes (`file:`, `javascript:`, `chrome:` and the like) reach past the
/// web, so they are refused before any browser starts.
fn web_url(args: &Map<String, Value>) -> Result<String, CallError> {
    let invalid = |why: String| CallError::InvalidArguments(why);
    let url = match args.get("url") {
        Some(Value::String(url)) => url,
        Some(other) => {
            return Err(invalid(format!(
                "`url` must be a string, an http:// or https:// URL, not {other}"
            )));
        }
        None => {
            return Err(invalid(
                "`url` is required: an http:// or https:// URL".to_owned(),
            ));
        }
    };
    if has_host(url) {
        Ok(url.clone())
    } else {
        Err(invalid(format!(
            "`url` must be an http:// or https:// URL with a host, not {url:?}"
        )))
    }
}

fn has_host(url: &str) -> bool {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let host = match host_port.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => host_port.split(':').next().unwrap_or_default(),
    };
    !host.is_empty()
}
