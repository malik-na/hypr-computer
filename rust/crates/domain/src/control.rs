//! What the `hypr-computer` commands and the daemon say to each other on the
//! daemon's control socket: one [`Command`] line, one [`Reply`] line, then
//! the daemon closes the connection.

/// A user's command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show the agents' live views.
    Watch,
    /// Stop every agent, and every one that connects until `Resume`.
    Stop,
    /// Let every agent go on.
    Resume,
}

impl Command {
    pub fn line(self) -> &'static str {
        match self {
            Command::Watch => "watch\n",
            Command::Stop => "stop\n",
            Command::Resume => "resume\n",
        }
    }

    pub fn parse(line: &str) -> Option<Self> {
        match line.trim_end() {
            "watch" => Some(Command::Watch),
            "stop" => Some(Command::Stop),
            "resume" => Some(Command::Resume),
            _ => None,
        }
    }
}

/// The daemon's answer, in words for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Done(String),
    /// It couldn't be done, and why.
    Refused(String),
}

impl Reply {
    pub fn line(&self) -> String {
        let (word, text) = match self {
            Reply::Done(text) => ("done", text),
            Reply::Refused(text) => ("refused", text),
        };
        format!("{word} {}\n", text.replace(['\n', '\r'], " "))
    }

    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim_end();
        if let Some(text) = line.strip_prefix("done ") {
            return Some(Reply::Done(text.to_owned()));
        }
        line.strip_prefix("refused ")
            .map(|text| Reply::Refused(text.to_owned()))
    }
}
