//! The app's log: one JSON object per line, written as each thing happens, so
//! a test can read it while the app runs.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use serde_json::Value;

pub struct Log {
    file: Option<File>,
}

impl Log {
    /// Appends to `path`; with no path, writes nothing.
    pub fn open(path: Option<&Path>) -> io::Result<Log> {
        let file = match path {
            Some(path) => Some(OpenOptions::new().create(true).append(true).open(path)?),
            None => None,
        };
        Ok(Log { file })
    }

    /// Writes `event` as one line, in one go, so a reader never sees half.
    pub fn write(&mut self, event: &Value) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.write_all(format!("{event}\n").as_bytes()),
            None => Ok(()),
        }
    }
}
