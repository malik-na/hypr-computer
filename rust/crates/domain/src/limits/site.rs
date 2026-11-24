//! Sites, by host: what `--sites` takes, and which pages are on a site.

use core::net::{Ipv4Addr, Ipv6Addr};

/// The longest host name DNS allows, and its longest label.
const MAX_HOST: usize = 253;
const MAX_LABEL: usize = 63;

/// A site as `--sites` takes it, reduced to its host: lower case, without a scheme, user, port, path,
/// query, fragment, trailing dot or IPv6 brackets. `None` if no host is left, or it has a character a host
/// can't have. A backslash ends the host, as it does in a browser's address bar.
pub fn normalize_site(text: &str) -> Option<String> {
    let text = text.to_lowercase();
    let rest = match text.split_once("://") {
        Some((scheme, rest)) if is_scheme(scheme) => rest,
        _ => text.as_str(),
    };
    let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or_default();
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(bracketed) = host_port.strip_prefix('[') {
        let (inside, after) = bracketed.split_once(']')?;
        if !after.is_empty() && !after.strip_prefix(':').is_some_and(is_port) {
            return None;
        }
        return inside.parse::<Ipv6Addr>().ok().map(|ip| ip.to_string());
    }
    if host_port.matches(':').count() > 1 {
        // A bare IPv6 address.
        return host_port.parse::<Ipv6Addr>().ok().map(|ip| ip.to_string());
    }
    let host = match host_port.split_once(':') {
        Some((host, port)) if is_port(port) => host,
        Some(_) => return None,
        None => host_port,
    };
    let host = host.strip_suffix('.').unwrap_or(host);
    is_host_name(host).then(|| host.to_owned())
}

/// Whether a page on `host` is on `site` (normalized): the site itself or a subdomain of it. An IP address
/// matches only itself. Both are compared as [`normalize_site`] reduces them.
pub fn site_matches(site: &str, host: &str) -> bool {
    let (Some(site), Some(host)) = (normalize_site(site), normalize_site(host)) else {
        return false;
    };
    if is_ip(&site) || is_ip(&host) {
        return site == host;
    }
    host == site
        || host
            .strip_suffix(site.as_str())
            .is_some_and(|sub| sub.ends_with('.'))
}

fn is_scheme(text: &str) -> bool {
    text.starts_with(|c: char| c.is_ascii_alphabetic())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Digits for a port; nothing at all is fine too, as in `example.com:`.
fn is_port(text: &str) -> bool {
    text.is_empty() || (text.len() <= 5 && text.parse::<u16>().is_ok())
}

/// A DNS name or an IPv4 address: labels of letters, digits, `-` and `_`, none empty, none starting or
/// ending with `-`.
fn is_host_name(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= MAX_HOST
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= MAX_LABEL
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
        })
}

fn is_ip(host: &str) -> bool {
    host.parse::<Ipv4Addr>().is_ok() || host.parse::<Ipv6Addr>().is_ok()
}
