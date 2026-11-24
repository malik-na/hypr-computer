//! What the `hypr-computer` commands and the daemon say to each other on the
//! daemon's control socket: one [`Command`] line, one [`Reply`] line, then
//! the daemon closes the connection.

/// A user's command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show the agents' live views.
    Watch,
    /// Stop every agent, and every one that connects until `Resume`.
    Stop,
    /// Let every agent go on.
    Resume,
    /// Stop the agents with this name, or this session number.
    StopOne(String),
    /// Let the agents with this name, or this session number, go on.
    ResumeOne(String),
    /// Hand back the screen the user took over.
    Give,
    /// Take over the screen of the agent with this name, or this session
    /// number.
    Take(String),
}

impl Command {
    pub fn line(&self) -> String {
        let one =
            |word: &str, agent: &str| format!("{word} {}\n", agent.replace(['\n', '\r'], " "));
        match self {
            Command::Watch => "watch\n".to_owned(),
            Command::Stop => "stop\n".to_owned(),
            Command::Resume => "resume\n".to_owned(),
            Command::Give => "give\n".to_owned(),
            Command::Take(agent) => one("take", agent),
            Command::StopOne(agent) => one("stop", agent),
            Command::ResumeOne(agent) => one("resume", agent),
        }
    }

    /// A line as read, or the words of the command line, joined by spaces.
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim_end();
        let (word, agent) = line.split_once(' ').unwrap_or((line, ""));
        match (word, agent.trim()) {
            ("watch", "") => Some(Command::Watch),
            ("stop", "") => Some(Command::Stop),
            ("resume", "") => Some(Command::Resume),
            ("give", "") => Some(Command::Give),
            ("take", agent) if !agent.is_empty() => Some(Command::Take(agent.to_owned())),
            ("stop", agent) => Some(Command::StopOne(agent.to_owned())),
            ("resume", agent) => Some(Command::ResumeOne(agent.to_owned())),
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
