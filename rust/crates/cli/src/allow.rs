//! `hypr-computer allow`: the limits every agent works within. They are kept
//! in a file in the runtime dir (`domain::paths::limits_file`), which the
//! daemon reads at every check, so this works whether or not a daemon runs,
//! and the limits hold across a restart of the daemon.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::process::ExitCode;
use std::time::SystemTime;

use domain::limits::{AllowCommand, Limits};
use domain::paths;

/// Runs `hypr-computer allow <args>`. `usage` is shown with a bad argument.
pub fn run(args: &[String], usage: &str) -> ExitCode {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let command = match AllowCommand::parse(&args) {
        Ok(command) => command,
        Err(why) => {
            eprintln!("hypr-computer allow: {why}\n\n{usage}");
            return ExitCode::from(2);
        }
    };
    let file = match paths::limits_file(std::env::var_os("XDG_RUNTIME_DIR").as_deref()) {
        Ok(file) => file,
        Err(e) => return fail(&e.to_string()),
    };
    let now = SystemTime::now();
    let done = match command {
        AllowCommand::List => read(&file).map(|limits| limits.describe(now)),
        AllowCommand::Off => write(&file, &Limits::none())
            .map(|()| format!("Limits lifted. {}", Limits::none().describe(now))),
        AllowCommand::Set(allow) => {
            let limits = Limits::set(&allow, now);
            write(&file, &limits).map(|()| limits.describe(now))
        }
    };
    match done {
        Ok(said) => {
            println!("{said}");
            ExitCode::SUCCESS
        }
        Err(why) => fail(&why),
    }
}

/// The limits in `file`; no file is no limit.
fn read(file: &Path) -> Result<Limits, String> {
    match fs::read_to_string(file) {
        Ok(text) => Limits::from_record(&text).ok_or_else(|| {
            format!(
                "The limits in {} are damaged, so agents are read-only. Run `hypr-computer allow` \
                 with new limits, or `hypr-computer allow --off`.",
                file.display()
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Limits::none()),
        Err(e) => Err(format!("can't read the limits in {}: {e}", file.display())),
    }
}

/// Replaces the limits in `file` with `limits`, all at once: the daemon
/// reads either the old ones or the new ones, never half of either.
fn write(file: &Path, limits: &Limits) -> Result<(), String> {
    let failed = |e: io::Error| format!("can't save the limits in {}: {e}", file.display());
    let Some(dir) = file.parent() else {
        return Err(format!("{} has no directory", file.display()));
    };
    // The daemon's own directory, private to the user, which it makes if it starts first.
    match DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(failed(e)),
        _ => {}
    }
    if !fs::symlink_metadata(dir).map_err(failed)?.is_dir() {
        return Err(format!("{} isn't a directory", dir.display()));
    }
    let new = dir.join(format!(".limits.{}", std::process::id()));
    let saved = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&new)
        .and_then(|mut out| {
            out.write_all(limits.record().as_bytes())?;
            out.sync_all()
        })
        .and_then(|()| fs::rename(&new, file));
    if let Err(e) = saved {
        let _ = fs::remove_file(&new);
        return Err(failed(e));
    }
    Ok(())
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::FAILURE
}
