//! What the user types: `hypr-computer allow <args>`.

use std::collections::BTreeSet;
use std::time::Duration;

use super::{Allow, normalize_site};

/// The longest `--for`: a hundred years. Anything longer can't be added to a clock everywhere.
const LONGEST: u64 = 100 * 365 * 24 * 3600;

/// `hypr-computer allow <args>`: what the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AllowCommand {
    /// Replace the limits with these.
    Set(Allow),
    /// `--list`, or no arguments: show the limits in force.
    List,
    /// `--off`: no limits.
    Off,
}

impl AllowCommand {
    /// Parses the arguments after `allow`. `Err` says, in words for the user, what is wrong with them,
    /// naming the offending argument where there is one.
    pub fn parse(args: &[&str]) -> Result<AllowCommand, String> {
        if args.is_empty() {
            return Ok(AllowCommand::List);
        }
        let (mut list, mut off, mut read_only) = (false, false, false);
        let mut apps = BTreeSet::new();
        let mut sites: Option<BTreeSet<String>> = None;
        let mut duration = None;
        let mut rest = args.iter().copied();
        while let Some(arg) = rest.next() {
            let (option, inline) = match arg.split_once('=') {
                Some((option, value)) if option.starts_with("--") => (option, Some(value)),
                _ => (arg, None),
            };
            match option {
                "--list" | "--off" | "--read-only" if inline.is_some() => {
                    return Err(format!("{option} takes no value: {arg}"));
                }
                "--list" => list = true,
                "--off" => off = true,
                "--read-only" => read_only = true,
                "--sites" => {
                    let value = inline
                        .or_else(|| rest.next())
                        .ok_or("--sites needs a list of sites, like --sites example.com,docs.rs")?;
                    let pieces: Vec<&str> = value.split(',').filter(|p| !p.is_empty()).collect();
                    if pieces.is_empty() {
                        return Err(
                            "--sites needs at least one site, like --sites example.com".to_owned()
                        );
                    }
                    let listed = sites.get_or_insert_with(BTreeSet::new);
                    for piece in pieces {
                        let host = normalize_site(piece).ok_or_else(|| {
                            format!(
                                "{piece} isn't a site: give a host like example.com, or a URL \
                                 like https://example.com"
                            )
                        })?;
                        listed.insert(host);
                    }
                }
                "--for" => {
                    if duration.is_some() {
                        return Err("--for is given twice; give it once".to_owned());
                    }
                    let value = inline
                        .or_else(|| rest.next())
                        .ok_or("--for needs a duration, like --for 30m")?;
                    duration = Some(parse_duration(value).ok_or_else(|| {
                        format!(
                            "{value} isn't a duration: give hours, minutes and seconds, like 30m, \
                             2h or 1h30m"
                        )
                    })?);
                }
                _ if arg.starts_with('-') => return Err(format!("unknown option {arg}")),
                _ => {
                    for piece in arg.split(',').filter(|p| !p.is_empty()) {
                        if piece.contains('/') {
                            return Err(format!(
                                "{piece}: name an app by its program's file name (like foot), \
                                 not by a path"
                            ));
                        }
                        if piece.chars().any(char::is_control) {
                            return Err(format!("{piece:?} isn't an app's name"));
                        }
                        apps.insert(piece.to_lowercase());
                    }
                }
            }
        }
        if (list || off) && args.len() > 1 {
            return Err("--list and --off take nothing else".to_owned());
        }
        if list {
            return Ok(AllowCommand::List);
        }
        if off {
            return Ok(AllowCommand::Off);
        }
        if read_only && (!apps.is_empty() || sites.is_some()) {
            return Err(
                "--read-only can't be given with apps or sites: read-only already lets agents \
                 only look"
                    .to_owned(),
            );
        }
        if apps.is_empty() && sites.is_none() && !read_only {
            return Err(
                "nothing to allow: name the apps agents may use, or give --sites or --read-only"
                    .to_owned(),
            );
        }
        Ok(AllowCommand::Set(Allow {
            apps: (!apps.is_empty()).then_some(apps),
            sites,
            read_only,
            duration,
        }))
    }
}

/// `30m`, `2h`, `45s`, `1h30m`, `1h30m15s`: hours, minutes and seconds in that order, each at most once,
/// digits only. `None` for anything else, for zero, and for more than a hundred years.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let mut rest = text;
    let mut total: u64 = 0;
    // The last unit read: hours 1, minutes 2, seconds 3.
    let mut last = 0;
    while !rest.is_empty() {
        let digits = rest.find(|c: char| !c.is_ascii_digit())?;
        if digits == 0 {
            return None;
        }
        let (number, tail) = rest.split_at(digits);
        let mut chars = tail.chars();
        let (rank, unit) = match chars.next()? {
            'h' => (1, 3600),
            'm' => (2, 60),
            's' => (3, 1),
            _ => return None,
        };
        if rank <= last {
            return None;
        }
        last = rank;
        let secs = number.parse::<u64>().ok()?.checked_mul(unit)?;
        total = total.checked_add(secs)?;
        rest = chars.as_str();
    }
    (total > 0 && total <= LONGEST).then(|| Duration::from_secs(total))
}
