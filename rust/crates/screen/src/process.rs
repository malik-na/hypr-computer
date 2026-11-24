//! The daemon's side of a screen: starting the process, asking it for
//! pixels, and tearing it down.

use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use domain::SessionId;
use ports::{BoxFuture, Png, Screen, ScreenError, ScreenProcess, Screens};
use rustix::io::Errno;
use rustix::process::{Pid, Signal, kill_process_group, test_kill_process_group};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, watch};
use tokio::time::timeout;
use tracing::warn;

use crate::SIZE;
use crate::protocol::{READY, Reply, Request};

/// How long a screen may take to say it is up.
const START_TIME: Duration = Duration::from_secs(10);
/// How long a screen may take to answer a request.
const ANSWER_TIME: Duration = Duration::from_secs(10);
/// The largest reply read from a screen; a screenshot is far smaller.
const MAX_REPLY: usize = 64 << 20;
/// The Wayland socket's file name, in the screen's own directory.
const SOCKET: &str = "wayland";

/// Starts screens as `hypr-computer-screen` processes, each with its files in
/// a directory of its own, `<dir>/screen-<session number>/`.
#[derive(Debug)]
pub struct ScreenProcesses {
    program: PathBuf,
    dir: PathBuf,
}

impl ScreenProcesses {
    pub fn new(program: PathBuf, dir: PathBuf) -> Self {
        Self { program, dir }
    }
}

impl Screens for ScreenProcesses {
    fn spawn(&self, session: SessionId) -> Result<Arc<dyn ScreenProcess>, ScreenError> {
        let dir = self.dir.join(format!("screen-{}", session.get()));
        fresh_private_dir(&dir)
            .map_err(|e| ScreenError(format!("can't create {}: {e}", dir.display())))?;
        let failed = |why: String| {
            let _ = fs::remove_dir_all(&dir);
            ScreenError(why)
        };
        let mut child = Command::new(&self.program)
            .arg(dir.join(SOCKET))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // A group of its own, so a teardown kills it and all it starts.
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| failed(format!("can't start {}: {e}", self.program.display())))?;
        let (Some(pid), Some(stdin), Some(stdout)) =
            (child.id(), child.stdin.take(), child.stdout.take())
        else {
            return Err(failed(
                "the screen process vanished as it started".to_owned(),
            ));
        };
        let group = i32::try_from(pid)
            .ok()
            .and_then(Pid::from_raw)
            .ok_or_else(|| failed(format!("the screen got an impossible pid, {pid}")))?;

        let (gone, exited) = watch::channel(false);
        tokio::spawn(async move {
            let _ = child.wait().await;
            gone.send_replace(true);
        });
        Ok(Arc::new(ScreenChild {
            pid,
            group,
            dir,
            pipe: Mutex::new(Some(Pipe {
                stdin,
                stdout: BufReader::new(stdout),
            })),
            exited,
        }))
    }
}

/// Removes what a daemon that died left at `dir`, then makes it afresh. No
/// screen can still be using it: only one daemon runs at a time.
fn fresh_private_dir(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    fs::DirBuilder::new().mode(0o700).create(dir)
}

struct ScreenChild {
    pid: u32,
    group: Pid,
    dir: PathBuf,
    /// `None` once the screen broke the protocol or stopped answering: the
    /// pipe may be out of step, so nothing more is asked of it.
    pipe: Mutex<Option<Pipe>>,
    exited: watch::Receiver<bool>,
}

struct Pipe {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Pipe {
    async fn read_line(&mut self) -> Result<String, ScreenError> {
        let mut line = String::new();
        match self.stdout.read_line(&mut line).await {
            Ok(0) => Err(ScreenError("the screen closed its output".to_owned())),
            Ok(_) => Ok(line),
            Err(e) => Err(ScreenError(format!("can't read from the screen: {e}"))),
        }
    }

    /// Sends `request`; `Err` if the pipe broke, `Ok(Err)` if the screen
    /// answered with an error.
    async fn ask(&mut self, request: Request) -> Result<Result<Vec<u8>, String>, ScreenError> {
        let wrote = async {
            self.stdin.write_all(request.line().as_bytes()).await?;
            self.stdin.flush().await
        };
        wrote
            .await
            .map_err(|e| ScreenError(format!("can't write to the screen: {e}")))?;
        let line = self.read_line().await?;
        match Reply::parse(&line) {
            Some(Reply::Png(len)) if len <= MAX_REPLY => {
                let mut bytes = vec![0; len];
                self.stdout
                    .read_exact(&mut bytes)
                    .await
                    .map_err(|e| ScreenError(format!("can't read from the screen: {e}")))?;
                Ok(Ok(bytes))
            }
            Some(Reply::Error(why)) => Ok(Err(why)),
            _ => Err(ScreenError(format!(
                "the screen answered {:?}",
                line.trim_end()
            ))),
        }
    }
}

impl ScreenChild {
    async fn ask(&self, request: Request) -> Result<Vec<u8>, ScreenError> {
        let mut pipe = self.pipe.lock().await;
        let Some(open) = pipe.as_mut() else {
            return Err(ScreenError("the screen stopped answering".to_owned()));
        };
        match timeout(ANSWER_TIME, open.ask(request)).await {
            Ok(Ok(answer)) => answer.map_err(ScreenError),
            Ok(Err(broke)) => {
                *pipe = None;
                Err(broke)
            }
            Err(_) => {
                *pipe = None;
                Err(ScreenError(format!(
                    "the screen didn't answer within {}s",
                    ANSWER_TIME.as_secs()
                )))
            }
        }
    }
}

impl Screen for ScreenChild {
    fn screenshot(&self) -> BoxFuture<'_, Result<Png, ScreenError>> {
        Box::pin(async move { self.ask(Request::Screenshot).await.map(Png) })
    }
}

impl ScreenProcess for ScreenChild {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn started(&self) -> BoxFuture<'_, Result<(), ScreenError>> {
        Box::pin(async move {
            let mut pipe = self.pipe.lock().await;
            let Some(open) = pipe.as_mut() else {
                return Err(ScreenError("the screen stopped answering".to_owned()));
            };
            let said = match timeout(START_TIME, open.read_line()).await {
                Ok(Ok(line)) if line.trim_end() == READY => return Ok(()),
                Ok(Ok(line)) => ScreenError(format!(
                    "the screen said {:?} instead of {READY:?}",
                    line.trim_end()
                )),
                Ok(Err(e)) => e,
                Err(_) => ScreenError(format!(
                    "the screen didn't start within {}s",
                    START_TIME.as_secs()
                )),
            };
            *pipe = None;
            Err(said)
        })
    }

    fn verify(&self) -> BoxFuture<'_, Result<(), ScreenError>> {
        Box::pin(async move {
            let shot = self.ask(Request::Screenshot).await?;
            match png_size(&shot) {
                Some(size) if size == SIZE => Ok(()),
                Some((w, h)) => Err(ScreenError(format!(
                    "the screen is {w}x{h}, not {}x{}",
                    SIZE.0, SIZE.1
                ))),
                None => Err(ScreenError(
                    "the screen's screenshot isn't a PNG".to_owned(),
                )),
            }
        })
    }

    fn exited(&self) -> BoxFuture<'_, ()> {
        let mut exited = self.exited.clone();
        Box::pin(async move {
            // An error means the watcher is gone, which it is only once the process is.
            let _ = exited.wait_for(|gone| *gone).await;
        })
    }

    fn teardown(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            // Until nothing is left in the group: the screen, reaped, and all it started.
            while test_kill_process_group(self.group) != Err(Errno::SRCH) {
                let _ = kill_process_group(self.group, Signal::KILL);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            // A killed screen leaves its socket and lock file behind.
            if let Err(e) = fs::remove_dir_all(&self.dir)
                && e.kind() != io::ErrorKind::NotFound
            {
                warn!("can't remove {}: {e}", self.dir.display());
            }
        })
    }
}

/// The width and height in a PNG's header.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if png.get(..8)? != SIGNATURE || png.get(12..16)? != b"IHDR" {
        return None;
    }
    let be = |at: usize| {
        png.get(at..at + 4)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_be_bytes)
    };
    Some((be(16)?, be(20)?))
}
