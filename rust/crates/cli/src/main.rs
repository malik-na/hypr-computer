//! hypr-computer: the user's commands. The daemon owns every agent's
//! session, so each command asks it, on its control socket, and prints the
//! answer. Without a daemon, no agent is working.
#![forbid(unsafe_code)]

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::Duration;

use domain::control::{Command, Reply};
use domain::paths;

/// How long the daemon may take to answer.
const ANSWER_TIME: Duration = Duration::from_secs(10);

const USAGE: &str = "usage: hypr-computer <command>

  watch    show the agents' screens (workspace 10)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match args.as_slice() {
        [name] if Command::parse(name).is_some() => Command::parse(name),
        [help] if ["help", "-h", "--help"].contains(&help.as_str()) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => None,
    };
    let Some(command) = command else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match ask(command) {
        Ok(Reply::Done(text)) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Ok(Reply::Refused(text)) => fail(&text),
        Err(why) => fail(&why),
    }
}

fn ask(command: Command) -> Result<Reply, String> {
    let socket = paths::control_socket(std::env::var_os("XDG_RUNTIME_DIR").as_deref())
        .map_err(|e| e.to_string())?;
    let mut stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err("No agent is working: hypr-computer's daemon isn't running.".to_owned());
        }
        Err(e) => {
            return Err(format!(
                "can't reach the daemon at {}: {e}",
                socket.display()
            ));
        }
    };
    let broke = |e: io::Error| format!("the daemon didn't answer: {e}");
    stream.set_read_timeout(Some(ANSWER_TIME)).map_err(broke)?;
    stream.write_all(command.line().as_bytes()).map_err(broke)?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(broke)?;
    Reply::parse(&line).ok_or_else(|| format!("the daemon answered {:?}", line.trim_end()))
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::FAILURE
}
