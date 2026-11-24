//! The filtering proxy the agents' browsers go through (#32): an HTTP proxy
//! on 127.0.0.1 that lets a request through only to a site the user's limits
//! allow at that moment (`Limits::allows_site`), asked at every request.
//!
//! It takes whole-URL requests (`GET http://host/path`), which it passes on
//! one per connection, and `CONNECT host:port` tunnels for https, checked
//! before anything is connected. It goes by the host in the request line,
//! never the `Host` header, and connects to the very host it checked; it
//! resolves the name only after the check. Anything else gets an error and
//! connects nowhere. A refused request gets 403 and a page saying so.
//!
//! Written by hand on tokio, not with an HTTP library: it only needs the
//! request line and the headers, and keeping the parsing small and strict
//! keeps what decides where a request goes easy to read.

mod request;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use domain::limits::Refusal;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, copy, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream, lookup_host};
use tokio::time::timeout;
use tracing::{info, warn};

use crate::limits::LimitsFile;
use request::{Kind, Request};

/// The largest request or response head read; more is refused.
const MAX_HEAD: usize = 64 * 1024;
/// How long a client may take to send its request's head.
const HEAD_TIME: Duration = Duration::from_secs(30);
/// How long connecting to a site may take.
const CONNECT_TIME: Duration = Duration::from_secs(30);

/// Serves the proxy on `listener` until the task is dropped.
pub async fn serve(listener: TcpListener, limits: Arc<LimitsFile>) {
    let own = match listener.local_addr() {
        Ok(own) => own,
        Err(e) => {
            warn!("the proxy has no address: {e}");
            return;
        }
    };
    info!("proxy listening on {own}");
    loop {
        match listener.accept().await {
            Ok((client, _)) => {
                tokio::spawn(handle(client, own, limits.clone()));
            }
            Err(e) => {
                warn!("accept on the proxy failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle(mut client: TcpStream, own: SocketAddr, limits: Arc<LimitsFile>) {
    let (head, rest) = match timeout(HEAD_TIME, read_head(&mut client)).await {
        Ok(Ok(Some(read))) => read,
        Ok(Ok(None)) => {
            answer(&mut client, 431, "the request's head is too long").await;
            return;
        }
        // It went away, or said nothing in time.
        _ => return,
    };
    let request = match Request::parse(&head) {
        Ok(request) => request,
        Err(why) => {
            answer(&mut client, 400, &why).await;
            return;
        }
    };
    let now = SystemTime::now();
    if !limits.load(now).allows_site(&request.host, now) {
        let why = Refusal::Site {
            host: request.host.clone(),
        };
        refuse(&mut client, &request.host, &why.to_string()).await;
        return;
    }
    let mut site = match connect(&request.host, request.port, own).await {
        Ok(site) => site,
        Err(why) => {
            answer(&mut client, 502, &why).await;
            return;
        }
    };
    match request.kind {
        Kind::Tunnel => {
            let opened = b"HTTP/1.1 200 Connection established\r\n\r\n";
            if client.write_all(opened).await.is_err() || site.write_all(&rest).await.is_err() {
                return;
            }
            let _ = copy_bidirectional(&mut client, &mut site).await;
        }
        Kind::Forward(head) => {
            if site.write_all(&head).await.is_err() || site.write_all(&rest).await.is_err() {
                return;
            }
            forward(client, site).await;
        }
    }
}

/// Reads a head, up to and with its blank line, and whatever came after it.
/// `None` if it is longer than [`MAX_HEAD`].
async fn read_head(
    from: &mut (impl AsyncRead + Unpin),
) -> std::io::Result<Option<(Vec<u8>, Vec<u8>)>> {
    let mut buffer = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        if let Some(end) = find(&buffer, b"\r\n\r\n") {
            let rest = buffer.split_off(end + 4);
            return Ok(Some((buffer, rest)));
        }
        if buffer.len() > MAX_HEAD {
            return Ok(None);
        }
        match from.read(&mut chunk).await? {
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            n => buffer.extend_from_slice(&chunk[..n]),
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Passes the rest of the request on to the site, and its answer back, with
/// the answer's head saying the connection closes after it: one request per
/// connection, so every request is checked.
async fn forward(client: TcpStream, site: TcpStream) {
    let (mut from_client, mut to_client) = client.into_split();
    let (mut from_site, mut to_site) = site.into_split();
    let upload = tokio::spawn(async move {
        let _ = copy(&mut from_client, &mut to_site).await;
    });
    let download = async {
        match read_head(&mut from_site).await {
            Ok(Some((head, rest))) => {
                to_client.write_all(&closing(&head)).await?;
                to_client.write_all(&rest).await?;
            }
            // A head too long to read: the connection is dropped.
            Ok(None) => return Ok(()),
            Err(e) => return Err(e),
        }
        copy(&mut from_site, &mut to_client).await?;
        to_client.shutdown().await
    };
    let _ = download.await;
    upload.abort();
}

/// A response head without the headers that keep a connection open, saying
/// it closes.
fn closing(head: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let mut out = String::new();
    for line in text.trim_end_matches("\r\n").split("\r\n") {
        let name = line.split(':').next().unwrap_or_default().trim();
        if ["connection", "proxy-connection", "keep-alive"]
            .iter()
            .any(|h| name.eq_ignore_ascii_case(h))
        {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\nProxy-Connection: close\r\n\r\n");
    out.into_bytes()
}

/// Connects to `host` on `port`, but never to the proxy itself.
async fn connect(host: &str, port: u16, own: SocketAddr) -> Result<TcpStream, String> {
    let addresses = timeout(CONNECT_TIME, lookup_host((host, port)))
        .await
        .map_err(|_| format!("looking up {host} took too long"))?
        .map_err(|e| format!("can't find {host}: {e}"))?;
    let mut last = format!("{host} is this proxy");
    for address in addresses {
        if address.port() == own.port()
            && (address.ip().is_loopback() || address.ip().is_unspecified())
        {
            continue;
        }
        match timeout(CONNECT_TIME, TcpStream::connect(address)).await {
            Ok(Ok(site)) => return Ok(site),
            Ok(Err(e)) => last = format!("can't connect to {host}: {e}"),
            Err(_) => last = format!("connecting to {host} took too long"),
        }
    }
    Err(last)
}

/// The proxy's page for a refused site: 403, naming the host.
async fn refuse(client: &mut TcpStream, host: &str, why: &str) {
    let host = escape(host);
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Blocked by hypr-computer: \
         {host}</title></head><body style=\"font-family:sans-serif;margin:3em\">\
         <h1>hypr-computer blocked {host}</h1><p>{}</p></body></html>",
        escape(why)
    );
    respond(client, 403, "Forbidden", &body).await;
}

/// An error page for a request the proxy didn't serve.
async fn answer(client: &mut TcpStream, status: u16, why: &str) {
    let reason = match status {
        400 => "Bad Request",
        431 => "Request Header Fields Too Large",
        _ => "Bad Gateway",
    };
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>hypr-computer's proxy</title>\
         </head><body><h1>hypr-computer's proxy: {reason}</h1><p>{}</p></body></html>",
        escape(why)
    );
    respond(client, status, reason, &body).await;
}

async fn respond(client: &mut TcpStream, status: u16, reason: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\
         Proxy-Connection: close\r\n\r\n",
        body.len()
    );
    let _ = client.write_all(head.as_bytes()).await;
    let _ = client.write_all(body.as_bytes()).await;
    let _ = client.shutdown().await;
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
