//! Accepting connections and running one MCP session on each, until SIGTERM
//! or SIGINT.

use std::sync::Arc;
use std::time::Duration;

use domain::EndReason;
use ports::{Clock, Desktop, Screens};
use rmcp::ServiceExt;
use rmcp::service::QuitReason;
use tokio::net::UnixStream;
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tools::Registry;
use tracing::{info, warn};

use crate::control;
use crate::limits::LimitsFile;
use crate::listen::Listening;
use crate::mcp::SessionServer;
use crate::owner::Owner;

/// How long sessions get to wind down once the daemon is told to stop.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// Runs until `stop` is cancelled, which `watch_signals` does on SIGTERM or
/// SIGINT.
pub async fn run(
    listening: Listening,
    clock: Arc<dyn Clock>,
    screens: Arc<dyn Screens>,
    stop: CancellationToken,
    desktop: Arc<dyn Desktop>,
    limits: Arc<LimitsFile>,
) -> Result<(), String> {
    let Listening {
        listener,
        owned_path,
        control,
        control_path,
        ..
    } = listening;
    let listener = into_tokio(listener)?;
    let control = into_tokio(control)?;

    let (owner, owner_task) = Owner::spawn(clock, screens, desktop, limits);
    let registry = Arc::new(Registry::builtin());
    let sessions = TaskTracker::new();
    let commands = tokio::spawn(control::serve(control, owner.clone()));

    loop {
        tokio::select! {
            () = stop.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    sessions.spawn(session(stream, owner.clone(), registry.clone(), stop.child_token()));
                }
                Err(e) => {
                    // Usually out of file descriptors; don't spin.
                    warn!("accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
        }
    }

    info!("stopping");
    drop(listener);
    commands.abort();
    for path in owned_path.iter().chain([&control_path]) {
        if let Err(e) = std::fs::remove_file(path) {
            warn!("can't remove {}: {e}", path.display());
        }
    }
    sessions.close();
    if tokio::time::timeout(STOP_GRACE, sessions.wait())
        .await
        .is_err()
    {
        warn!(
            "sessions still running after {}s; stopping anyway",
            STOP_GRACE.as_secs()
        );
    }
    // The owner ends once every session's screen is torn down.
    owner.stop();
    drop(owner);
    if tokio::time::timeout(STOP_GRACE, owner_task).await.is_err() {
        warn!(
            "screens still being torn down after {}s; stopping anyway",
            STOP_GRACE.as_secs()
        );
    }
    info!("stopped");
    Ok(())
}

fn into_tokio(
    listener: std::os::unix::net::UnixListener,
) -> Result<tokio::net::UnixListener, String> {
    listener
        .set_nonblocking(true)
        .and_then(|()| tokio::net::UnixListener::from_std(listener))
        .map_err(|e| format!("can't use the socket: {e}"))
}

/// Starts handling SIGTERM and SIGINT, which cancel the returned token. It
/// must run inside a tokio runtime, and before the daemon makes its socket
/// or says it listens: a signal sent any earlier than this would kill it
/// with the default action, leaving the socket behind and a non-zero exit.
/// A signal that comes after is held until the daemon starts serving.
pub fn watch_signals() -> Result<CancellationToken, String> {
    let stop = CancellationToken::new();
    let mut term =
        signal(SignalKind::terminate()).map_err(|e| format!("can't watch SIGTERM: {e}"))?;
    let mut int =
        signal(SignalKind::interrupt()).map_err(|e| format!("can't watch SIGINT: {e}"))?;
    let cancel = stop.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = term.recv() => info!("got SIGTERM"),
            _ = int.recv() => info!("got SIGINT"),
        }
        cancel.cancel();
    });
    Ok(stop)
}

/// One agent connection: its session ends when it closes, and the session
/// closes it when it ends first.
async fn session(
    stream: UnixStream,
    owner: Owner,
    registry: Arc<Registry>,
    stop: CancellationToken,
) {
    let connection = stop.child_token();
    let Some(ticket) = owner.open(connection.clone()).await else {
        return;
    };
    let server = SessionServer::new(Arc::new(ticket.gate()), registry);
    let reason = match server.serve_with_ct(stream, connection).await {
        Ok(running) => match running.waiting().await {
            Ok(QuitReason::Closed) => EndReason::Disconnected,
            // Otherwise the session ended first and closed it: the reason is moot.
            Ok(QuitReason::Cancelled) if stop.is_cancelled() => EndReason::DaemonStopping,
            Ok(other) => EndReason::Failed(format!("{other:?}")),
            Err(e) => EndReason::Failed(e.to_string()),
        },
        Err(_) if stop.is_cancelled() => EndReason::DaemonStopping,
        Err(rmcp::service::ServerInitializeError::ConnectionClosed(_)) => EndReason::Disconnected,
        Err(e) => EndReason::Failed(e.to_string()),
    };
    ticket.end(reason);
}
