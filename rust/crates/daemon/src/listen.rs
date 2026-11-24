//! Getting the listening socket: from systemd if it passed one, otherwise
//! bound at `$XDG_RUNTIME_DIR/hypr-computer/daemon.sock` in a 0700 dir.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use domain::paths;
use listenfd::ListenFd;

/// The socket the daemon accepts agents on.
#[derive(Debug)]
pub struct Listening {
    pub listener: UnixListener,
    /// The socket file to remove on exit; `None` when systemd owns it.
    pub owned_path: Option<PathBuf>,
}

impl Listening {
    pub fn describe(&self) -> String {
        match &self.owned_path {
            Some(p) => format!("listening on {}", p.display()),
            None => "listening on the socket passed by systemd".to_owned(),
        }
    }
}

pub fn listen(fds: &mut ListenFd, runtime_dir: Option<&OsStr>) -> Result<Listening, String> {
    if fds.len() > 0 {
        return match fds.take_unix_listener(0) {
            Ok(Some(listener)) => Ok(Listening {
                listener,
                owned_path: None,
            }),
            Ok(None) => Err("systemd passed no socket at fd 3".to_owned()),
            Err(e) => Err(format!(
                "systemd's socket at fd 3 isn't a listening unix socket: {e}"
            )),
        };
    }
    let dir = paths::daemon_dir(runtime_dir).map_err(|e| e.to_string())?;
    make_private_dir(&dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    let path = dir.join(paths::SOCKET_NAME);
    let listener = bind(&path)?;
    Ok(Listening {
        listener,
        owned_path: Some(path),
    })
}

fn make_private_dir(dir: &Path) -> io::Result<()> {
    fs::DirBuilder::new().mode(0o700).create(dir).or_else(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            Ok(())
        } else {
            Err(e)
        }
    })?;
    if !fs::symlink_metadata(dir)?.is_dir() {
        return Err(io::Error::other("it exists and is not a directory"));
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// Binds `path`, replacing a socket file left by a daemon that died, but
/// never one that a running daemon still answers on.
fn bind(path: &Path) -> Result<UnixListener, String> {
    let shown = path.display();
    match UnixListener::bind(path) {
        Ok(l) => Ok(l),
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            if UnixStream::connect(path).is_ok() {
                return Err(format!(
                    "a hypr-computer daemon is already running at {shown}"
                ));
            }
            fs::remove_file(path)
                .map_err(|e| format!("can't remove the stale socket {shown}: {e}"))?;
            UnixListener::bind(path).map_err(|e| format!("can't listen on {shown}: {e}"))
        }
        Err(e) => Err(format!("can't listen on {shown}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short_tempdir() -> PathBuf {
        let dir = std::env::temp_dir();
        // macOS's temp dir is too long for a socket path; /tmp is short everywhere.
        let base = if dir.as_os_str().len() > 40 {
            PathBuf::from("/tmp")
        } else {
            dir
        };
        let p = base.join(format!("hcd-{}-{}", std::process::id(), line!()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir(&p).unwrap();
        p
    }

    #[test]
    fn a_loose_existing_dir_is_tightened_to_0700() {
        let run = short_tempdir();
        let dir = run.join("hypr-computer");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        make_private_dir(&dir).unwrap();
        let mode = fs::metadata(&dir).unwrap().permissions().mode();
        fs::remove_dir_all(&run).unwrap();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn no_runtime_dir_and_no_systemd_socket_is_a_clear_error() {
        let err = listen(&mut ListenFd::empty(), None).unwrap_err();
        assert!(err.contains("XDG_RUNTIME_DIR is not set"), "{err}");
    }
}
