//! Screenshots in the log, opt-in (`[log] screenshots = true`): each
//! picture an agent gets is saved as it got it, a PNG file in a dir named
//! after its session's log, which goes with the log. With the setting off,
//! no picture reaches the disk.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::sync::atomic::Ordering;

use ports::Png;
use tracing::warn;

use super::{SessionLog, make_private};

impl SessionLog {
    /// Saves `png` if the user wants pictures kept; where, relative to the
    /// log dir.
    pub(super) fn save(&self, png: &Png) -> Option<String> {
        if !self.log.screenshots {
            return None;
        }
        let n = self.pictures.fetch_add(1, Ordering::Relaxed) + 1;
        let saved = format!("{}/{n}.png", self.name);
        match self.write(&saved, png) {
            Ok(()) => Some(saved),
            Err(e) => {
                warn!("can't save a screenshot in {}: {e}", self.log.dir.display());
                None
            }
        }
    }

    fn write(&self, saved: &str, png: &Png) -> io::Result<()> {
        self.log.private_dirs()?;
        let dir = self.log.dir.join(&self.name);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .or_else(|e| match e.kind() {
                io::ErrorKind::AlreadyExists => Ok(()),
                _ => Err(e),
            })?;
        make_private(&dir, 0o700)?;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.log.dir.join(saved))?
            .write_all(&png.0)
    }
}
