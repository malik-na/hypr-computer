//! Accepting connections and running one MCP session on each, until SIGTERM
//! or SIGINT.

use std::sync::Arc;
use std::time::Duration;

use domain::EndReason;
use ports::{Clock, Screens};
use rmcp::ServiceExt;
use rmcp::service::QuitReason;
use tokio::net::UnixStream;
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tools::Registry;
use tracing::{info, warn};

use crate::listen::Listening;
use crate::mcp::SessionServer;
use crate::owner::Owner;

/// How long sessions get to wind down once the daemon is told to stop.
const STOP_GRACE: Duration = Duration::from_secs(5);

pub async fn run(
    listening: Listening,
    clock: Arc<dyn Clock>,
    screens: Arc<dyn Screens>,
) -> Result<(), String> {
    let Listening {
        listener,
        owned_path,
        ..
    } = listening;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("can't use the socket: {e}"))?;
    let listener = tokio::net::UnixListener::from_std(listener)
        .map_err(|e| format!("can't use the socket: {e}"))?;

    let stop = CancellationToken::new();
    watch_signals(stop.clone())?;
    let (owner, owner_task) = Owner::spawn(clock, screens);
    let registry = Arc::new(Registry::builtin());
    let sessions = TaskTracker::new();

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
    if let Some(path) = owned_path
        && let Err(e) = std::fs::remove_file(&path)
    {
        warn!("can't remove {}: {e}", path.display());
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

fn watch_signals(stop: CancellationToken) -> Result<(), String> {
    let mut term =
        signal(SignalKind::terminate()).map_err(|e| format!("can't watch SIGTERM: {e}"))?;
    let mut int =
        signal(SignalKind::interrupt()).map_err(|e| format!("can't watch SIGINT: {e}"))?;
    tokio::spawn(async move {
        tokio::select! {
            _ = term.recv() => info!("got SIGTERM"),
            _ = int.recv() => info!("got SIGINT"),
        }
        stop.cancel();
    });
    Ok(())
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
