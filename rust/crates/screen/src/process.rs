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
use ports::{
    Action, AppCommand, BoxFuture, Launched, Png, Screen, ScreenError, ScreenProcess, Screens,
};
use rustix::process::{Pid, Signal, kill_process_group, test_kill_process_group};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, watch};
use tokio::time::timeout;
use tracing::warn;

use crate::SIZE;
use crate::protocol::{READY, Reply, Request, Status};
use crate::settle::parts;

/// How long a screen may take to say it is up.
const START_TIME: Duration = Duration::from_secs(10);
/// How long a screen may take to answer a request.
const ANSWER_TIME: Duration = Duration::from_secs(10);
/// The largest reply read from a screen; a screenshot is far smaller.
const MAX_REPLY: usize = 64 << 20;
/// The Wayland socket's file name, in the screen's own directory.
const SOCKET: &str = "wayland";

/// Starts screens as `hypr-computer-screen` processes, each with its files in
/// a directory of its own, `<dir>/screen-<session number>/`, and its live
/// view on the user's desktop, the Wayland socket `desktop`, if there is one.
#[derive(Debug)]
pub struct ScreenProcesses {
    program: PathBuf,
    dir: PathBuf,
    desktop: Option<PathBuf>,
}

impl ScreenProcesses {
    pub fn new(program: PathBuf, dir: PathBuf, desktop: Option<PathBuf>) -> Self {
        Self {
            program,
            dir,
            desktop,
        }
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
            .args(&self.desktop)
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

pub(crate) struct ScreenChild {
    pid: u32,
    /// The screen's process group, which every app on it joins.
    pub(crate) group: Pid,
    /// The screen's own directory: its socket, and its apps' runtime dir.
    pub(crate) dir: PathBuf,
    /// `None` once the screen broke the protocol or stopped answering: the
    /// pipe may be out of step, so nothing more is asked of it.
    pipe: Mutex<Option<Pipe>>,
    exited: watch::Receiver<bool>,
}

pub(crate) struct Pipe {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// What the screen answered.
enum Answer {
    Png(Vec<u8>),
    Status(Status),
    Done(u64),
}

/// The screen answered, but not what was asked; `Pipe::ask` rules it out.
fn mismatch() -> ScreenError {
    ScreenError("the screen's answer doesn't fit the question".to_owned())
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

    /// Sends `request`; `Err` if the pipe broke or the answer doesn't fit
    /// the request, `Ok(Err)` if the screen answered with an error.
    async fn ask(&mut self, request: &Request) -> Result<Result<Answer, String>, ScreenError> {
        let wrote = async {
            self.stdin.write_all(request.line().as_bytes()).await?;
            self.stdin.flush().await
        };
        wrote
            .await
            .map_err(|e| ScreenError(format!("can't write to the screen: {e}")))?;
        let line = self.read_line().await?;
        match (request, Reply::parse(&line)) {
            (Request::Screenshot, Some(Reply::Png(len))) if len <= MAX_REPLY => {
                let mut bytes = vec![0; len];
                self.stdout
                    .read_exact(&mut bytes)
                    .await
                    .map_err(|e| ScreenError(format!("can't read from the screen: {e}")))?;
                Ok(Ok(Answer::Png(bytes)))
            }
            (Request::Status, Some(Reply::Status(status))) => Ok(Ok(Answer::Status(status))),
            (
                Request::Act(_) | Request::Press(_) | Request::Release(_) | Request::View(_),
                Some(Reply::Done(commits)),
            ) => Ok(Ok(Answer::Done(commits))),
            (_, Some(Reply::Error(why))) => Ok(Err(why)),
            _ => Err(ScreenError(format!(
                "the screen answered {:?} to {:?}",
                line.trim_end(),
                request.line().trim_end()
            ))),
        }
    }
}

impl ScreenChild {
    /// The Wayland socket apps connect to.
    pub(crate) fn socket(&self) -> PathBuf {
        self.dir.join(SOCKET)
    }

    /// The runtime dir apps on the screen get in place of the user's.
    pub(crate) fn app_runtime_dir(&self) -> PathBuf {
        self.dir.join("run")
    }

    pub(crate) async fn status(&self) -> Result<Status, ScreenError> {
        self.status_on(&mut *self.pipe.lock().await).await
    }

    /// The status, with the pipe already locked.
    pub(crate) async fn status_on(&self, pipe: &mut Option<Pipe>) -> Result<Status, ScreenError> {
        match self.ask_on(pipe, &Request::Status).await? {
            Answer::Status(status) => Ok(status),
            _ => Err(mismatch()),
        }
    }

    async fn png(&self) -> Result<Vec<u8>, ScreenError> {
        match self
            .ask_on(&mut *self.pipe.lock().await, &Request::Screenshot)
            .await?
        {
            Answer::Png(png) => Ok(png),
            _ => Err(mismatch()),
        }
    }

    /// Does `action`, then holds the pipe until the screen has settled, so
    /// no other call sees the screen before the app has drawn its response.
    async fn act_and_settle(&self, action: Action) -> Result<(), ScreenError> {
        let mut pipe = self.pipe.lock().await;
        let mut since = 0;
        for (n, part) in parts(action)?.into_iter().enumerate() {
            if n > 0 {
                self.wait_read(&mut pipe).await?;
            }
            since = match self.ask_on(&mut pipe, &part).await? {
                Answer::Done(commits) => commits,
                _ => return Err(mismatch()),
            };
        }
        self.settle(&mut pipe, since).await
    }

    async fn ask_on(
        &self,
        pipe: &mut Option<Pipe>,
        request: &Request,
    ) -> Result<Answer, ScreenError> {
        let Some(open) = pipe.as_mut() else {
            return Err(ScreenError("the screen stopped answering".to_owned()));
        };
        let broke = match timeout(ANSWER_TIME, open.ask(request)).await {
            Ok(Ok(answer)) => return answer.map_err(ScreenError),
            Ok(Err(broke)) => broke,
            Err(_) => ScreenError(format!(
                "the screen didn't answer within {}s",
                ANSWER_TIME.as_secs()
            )),
        };
        self.give_up(pipe, &broke);
        Err(broke)
    }

    /// The pipe can't be trusted any more. Stops using it, and kills the
    /// screen: its session then ends, as for any screen that exits, instead
    /// of failing every call from now on.
    fn give_up(&self, pipe: &mut Option<Pipe>, why: &ScreenError) {
        warn!(
            "screen {} stopped making sense ({why}); killing it",
            self.pid
        );
        *pipe = None;
        self.kill_group();
    }

    /// Sends SIGKILL to the screen's group; `false` once nothing in it is
    /// left that may be signalled. Until the screen is reaped, the group is
    /// surely its own. After that, it is only while something is still in it.
    fn kill_group(&self) -> bool {
        let reaped = *self.exited.borrow();
        if reaped && test_kill_process_group(self.group).is_err() {
            return false;
        }
        let _ = kill_process_group(self.group, Signal::KILL);
        true
    }
}

impl Screen for ScreenChild {
    fn screenshot(&self) -> BoxFuture<'_, Result<Png, ScreenError>> {
        Box::pin(async move { self.png().await.map(Png) })
    }

    fn act(&self, action: Action) -> BoxFuture<'_, Result<(), ScreenError>> {
        Box::pin(self.act_and_settle(action))
    }

    fn launch(&self, app: AppCommand) -> BoxFuture<'_, Result<Launched, ScreenError>> {
        Box::pin(self.start_app(app))
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
            let broke = match timeout(START_TIME, open.read_line()).await {
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
            self.give_up(&mut pipe, &broke);
            Err(broke)
        })
    }

    fn verify(&self) -> BoxFuture<'_, Result<(), ScreenError>> {
        Box::pin(async move {
            let shot = self.png().await?;
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

    fn open_view(&self, title: String) -> BoxFuture<'_, Result<(), ScreenError>> {
        Box::pin(async move {
            match self
                .ask_on(&mut *self.pipe.lock().await, &Request::View(title))
                .await?
            {
                Answer::Done(_) => Ok(()),
                _ => Err(mismatch()),
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
            // Until the screen is reaped and nothing it started is left, in
            // its process group or out of it.
            while self.kill_group() | self.kill_strays() {
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
