//! A request to the proxy, as it is parsed and passed on. Strict on purpose:
//! anything this doesn't understand is refused rather than guessed at, and
//! the host checked is the very host connected to.

use domain::limits::normalize_site;

/// Headers that are about the connection to the proxy, not the request, and
/// so aren't passed on. `Host` is written afresh from the request line.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "proxy-connection",
    "keep-alive",
    "proxy-authorization",
    "proxy-authenticate",
    "te",
    "trailer",
    "upgrade",
    "host",
];

/// A request the proxy can serve.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    /// The host in the request line, as `normalize_site` reduces it: what the
    /// limits are asked about, and what the proxy connects to.
    pub host: String,
    pub port: u16,
    pub kind: Kind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Kind {
    /// `CONNECT host:port`: a tunnel.
    Tunnel,
    /// `GET http://host/path` and the like: the head to send the site.
    Forward(Vec<u8>),
}

impl Request {
    /// Parses a request head, its blank line included. `Err` says why it
    /// isn't one the proxy serves.
    pub fn parse(head: &[u8]) -> Result<Request, String> {
        let head = std::str::from_utf8(head).map_err(|_| "the request isn't text".to_owned())?;
        let head = head
            .strip_suffix("\r\n\r\n")
            .ok_or("the request's head doesn't end")?;
        let mut lines = head.split("\r\n");
        let line = lines.next().unwrap_or_default();
        let [method, target, version] = words(line)?;
        if !is_token(method) {
            return Err(format!("{method:?} isn't a method"));
        }
        if version != "HTTP/1.1" && version != "HTTP/1.0" {
            return Err(format!("{version:?} isn't HTTP/1.x"));
        }
        let mut headers = Vec::new();
        for line in lines {
            let (name, value) = line
                .split_once(':')
                .filter(|(name, _)| is_token(name))
                .ok_or_else(|| format!("{line:?} isn't a header"))?;
            if value.chars().any(|c| c.is_control() && c != '\t') {
                return Err(format!("the header {name} has a control character"));
            }
            headers.push((name, value.trim()));
        }
        if method == "CONNECT" {
            let (host, port) = authority(target, None)?;
            return Ok(Request {
                host,
                port,
                kind: Kind::Tunnel,
            });
        }
        let rest = strip_prefix_ignore_case(target, "http://").ok_or_else(|| {
            format!("{target:?} isn't an http:// URL: this is a proxy, which takes whole URLs")
        })?;
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority_part, path) = rest.split_at(end);
        // A URL's `user@` part isn't its host.
        let host_port = authority_part
            .rsplit_once('@')
            .map_or(authority_part, |(_, h)| h);
        let (host, port) = authority(host_port, Some(80))?;
        let path = path.split('#').next().unwrap_or_default();
        let path = match path.chars().next() {
            Some('/') => path.to_owned(),
            _ => format!("/{path}"),
        };
        let mut out = format!("{method} {path} {version}\r\n");
        for (name, value) in headers {
            if !HOP_BY_HOP.contains(&name.to_ascii_lowercase().as_str()) {
                out.push_str(&format!("{name}: {value}\r\n"));
            }
        }
        let shown = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.clone()
        };
        if port == 80 {
            out.push_str(&format!("Host: {shown}\r\n"));
        } else {
            out.push_str(&format!("Host: {shown}:{port}\r\n"));
        }
        out.push_str("Connection: close\r\n\r\n");
        Ok(Request {
            host,
            port,
            kind: Kind::Forward(out.into_bytes()),
        })
    }
}

/// `host:port`, or `[v6]:port`; the port may be left out only if there is a
/// `default`. The host is reduced as `normalize_site` reduces it, and
/// anything that isn't plainly a host is refused.
fn authority(text: &str, default: Option<u16>) -> Result<(String, u16), String> {
    let bad = || format!("{text:?} isn't a host and port");
    let (host, port) = match text.strip_prefix('[') {
        Some(rest) => {
            let (inside, after) = rest.split_once(']').ok_or_else(bad)?;
            let port = match after {
                "" => None,
                after => Some(after.strip_prefix(':').ok_or_else(bad)?),
            };
            (inside, port)
        }
        None => match text.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (text, None),
        },
    };
    let port = match port {
        Some(port) => port
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(bad)?,
        None => default.ok_or_else(bad)?,
    };
    if host.is_empty() || host.contains(['@', '/', '\\', '?', '#', '%', '[', ']']) {
        return Err(bad());
    }
    let host = normalize_site(host).ok_or_else(bad)?;
    Ok((host, port))
}

/// Exactly three words, one space apart.
fn words(line: &str) -> Result<[&str; 3], String> {
    line.split(' ')
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| format!("{line:?} isn't a request line"))
}

/// A method or header name.
fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c))
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}
