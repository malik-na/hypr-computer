//! `hypr-computer log <agent>`: the log of the sessions of that name, this
//! daemon's and earlier ones', oldest first, or of that session number of
//! this daemon, as `stop <agent>` picks agents. One line per call.

use std::fs;
use std::path::PathBuf;

use domain::control::Reply;
use domain::view::agent_name;
use serde_json::{Map, Value};

use super::ActionLog;

/// A session's log, as read back.
struct Session {
    file: PathBuf,
    records: Vec<Map<String, Value>>,
}

impl Session {
    fn first(&self, field: &str) -> &Value {
        self.records
            .first()
            .and_then(|r| r.get(field))
            .unwrap_or(&Value::Null)
    }

    fn agent(&self) -> &str {
        self.first("agent").as_str().unwrap_or("")
    }
}

impl ActionLog {
    pub fn show(&self, agent: &str) -> Reply {
        let this_session = agent.parse().ok().map(|n| self.file(n));
        let name = agent_name(agent);
        let mut sessions: Vec<Session> = self
            .sessions()
            .into_iter()
            .filter(|s| Some(&s.file) == this_session.as_ref() || agent_name(s.agent()) == name)
            .collect();
        if sessions.is_empty() {
            return Reply::Refused(format!(
                "There is nothing in the action log for {agent:?}: give the name or the session \
                 number on its screen, as in \"claude-code (session 2)\". Logs are kept a few \
                 days."
            ));
        }
        sessions.sort_by(|a, b| {
            (a.first("time").as_str(), &a.file).cmp(&(b.first("time").as_str(), &b.file))
        });
        let shown: Vec<String> = sessions.iter().map(show).collect();
        Reply::Done(shown.join("\n\n"))
    }

    /// Every session in the log with at least one record.
    fn sessions(&self) -> Vec<Session> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .filter_map(|file| {
                let text = fs::read_to_string(&file).ok()?;
                // The last line may be half written.
                let records: Vec<Map<String, Value>> = text
                    .lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect();
                (!records.is_empty()).then_some(Session { file, records })
            })
            .collect()
    }
}

/// "alpha (session 1), started 2026-11-24 10:15:03 UTC", then a line per
/// call.
fn show(session: &Session) -> String {
    let time = session.first("time").as_str().unwrap_or("");
    let mut lines = vec![format!(
        "{} (session {}), started {} {} UTC",
        agent_name(session.agent()),
        session.first("session"),
        time.get(..10).unwrap_or("?"),
        time.get(11..19).unwrap_or("?"),
    )];
    lines.extend(session.records.iter().map(call_line));
    lines.join("\n")
}

/// "  10:15:03  click x=640 y=400  ok".
fn call_line(record: &Map<String, Value>) -> String {
    let field = |name: &str| record.get(name).and_then(Value::as_str).unwrap_or("");
    let arguments = record
        .get("arguments")
        .and_then(Value::as_object)
        .map(|args| {
            args.iter()
                .map(|(name, value)| format!("{}={}", printable(name), argument(value)))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let parts = [
        field("time").get(11..19).unwrap_or("?").to_owned(),
        printable(field("tool")),
        arguments,
        field("result").to_owned(),
    ];
    let parts: Vec<String> = parts.into_iter().filter(|p| !p.is_empty()).collect();
    format!("  {}", parts.join("  "))
}

/// An argument as recorded: a length as "23 characters", a plain word as
/// itself, anything else as JSON.
fn argument(value: &Value) -> String {
    if let Some(n) = value.get("characters").and_then(Value::as_u64) {
        return if n == 1 {
            "1 character".to_owned()
        } else {
            format!("{n} characters")
        };
    }
    match value.as_str() {
        Some(word) if !word.is_empty() && !word.contains(char::is_whitespace) => printable(word),
        _ => value.to_string(),
    }
}

/// `text` without control characters, which could fake lines of the log.
fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}
