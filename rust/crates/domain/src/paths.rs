//! Where the daemon listens, and where the user's desktop is. The daemon and
//! the shim both use this, so they can't disagree.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

/// The directory under `$XDG_RUNTIME_DIR` that holds the daemon's socket.
pub const DIR_NAME: &str = "hypr-computer";
/// The socket's file name inside [`DIR_NAME`].
pub const SOCKET_NAME: &str = "daemon.sock";
/// The file name of the socket the `hypr-computer` commands use, inside
/// [`DIR_NAME`].
pub const CONTROL_NAME: &str = "control.sock";
/// The file name of the limits `hypr-computer allow` sets, inside [`DIR_NAME`] (`limits::Limits::record`).
pub const LIMITS_NAME: &str = "limits";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeDirError {
    Unset,
    NotAbsolute(PathBuf),
}

impl fmt::Display for RuntimeDirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeDirError::Unset => f.write_str(
                "XDG_RUNTIME_DIR is not set; run this inside your login session (it is usually /run/user/$UID)",
            ),
            RuntimeDirError::NotAbsolute(p) => write!(
                f,
                "XDG_RUNTIME_DIR must be an absolute path, not {}",
                p.display()
            ),
        }
    }
}

impl std::error::Error for RuntimeDirError {}

/// The daemon's directory, given the value of `$XDG_RUNTIME_DIR`.
pub fn daemon_dir(runtime_dir: Option<&OsStr>) -> Result<PathBuf, RuntimeDirError> {
    let dir = match runtime_dir {
        Some(d) if !d.is_empty() => Path::new(d),
        _ => return Err(RuntimeDirError::Unset),
    };
    if !dir.is_absolute() {
        return Err(RuntimeDirError::NotAbsolute(dir.to_path_buf()));
    }
    Ok(dir.join(DIR_NAME))
}

/// The daemon's socket, given the value of `$XDG_RUNTIME_DIR`.
pub fn daemon_socket(runtime_dir: Option<&OsStr>) -> Result<PathBuf, RuntimeDirError> {
    Ok(daemon_dir(runtime_dir)?.join(SOCKET_NAME))
}

/// The socket the `hypr-computer` commands use, given the value of
/// `$XDG_RUNTIME_DIR`.
pub fn control_socket(runtime_dir: Option<&OsStr>) -> Result<PathBuf, RuntimeDirError> {
    Ok(daemon_dir(runtime_dir)?.join(CONTROL_NAME))
}

/// The limits `hypr-computer allow` sets, given the value of `$XDG_RUNTIME_DIR`: in the runtime dir, so
/// they hold across a restart of the daemon and end with the login session.
pub fn limits_file(runtime_dir: Option<&OsStr>) -> Result<PathBuf, RuntimeDirError> {
    Ok(daemon_dir(runtime_dir)?.join(LIMITS_NAME))
}

/// The user's desktop's Wayland socket, given the values of
/// `$WAYLAND_DISPLAY` and `$XDG_RUNTIME_DIR`, as Wayland apps find it:
/// `$WAYLAND_DISPLAY` if it is absolute, otherwise inside the runtime dir.
pub fn desktop_socket(
    wayland_display: Option<&OsStr>,
    runtime_dir: Option<&OsStr>,
) -> Option<PathBuf> {
    let display = Path::new(wayland_display.filter(|d| !d.is_empty())?);
    if display.is_absolute() {
        return Some(display.to_path_buf());
    }
    let dir = Path::new(runtime_dir.filter(|d| !d.is_empty())?);
    dir.is_absolute().then(|| dir.join(display))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_lives_in_its_own_dir_under_the_runtime_dir() {
        assert_eq!(
            daemon_socket(Some(OsStr::new("/run/user/1000"))),
            Ok(Path::new("/run/user/1000/hypr-computer/daemon.sock").to_path_buf())
        );
    }

    #[test]
    fn unset_or_empty_runtime_dir_is_an_error_that_says_what_to_do() {
        assert_eq!(daemon_socket(None), Err(RuntimeDirError::Unset));
        assert_eq!(
            daemon_socket(Some(OsStr::new(""))),
            Err(RuntimeDirError::Unset)
        );
        assert!(RuntimeDirError::Unset.to_string().contains("login session"));
    }

    #[test]
    fn relative_runtime_dir_is_refused() {
        assert_eq!(
            daemon_socket(Some(OsStr::new("run/user/1000"))),
            Err(RuntimeDirError::NotAbsolute(PathBuf::from("run/user/1000")))
        );
    }
}
