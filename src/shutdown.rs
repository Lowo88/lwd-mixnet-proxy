//! Waiting to be asked to stop.

/// Resolves the first time the process is asked to shut down.
///
/// Container runtimes ask with `SIGTERM` and escalate to `SIGKILL` once a grace period runs out, so
/// a process that waits on an interrupt alone never drains where these are meant to run: it is
/// killed outright, mid-connection, however long its own grace period was set to.
///
/// Poll it once and keep the future: each poll of a fresh one registers the handler again, and a
/// signal that arrives before the first poll is missed.
pub async fn requested() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = terminate.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(error) => {
                tracing::warn!(%error, "cannot listen for SIGTERM: only an interrupt will drain");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
