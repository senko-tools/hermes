//! Running the server until it is told to stop, then shutting down gracefully (PLAN.md 9.1).

use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinError;

use crate::state::AppState;
use crate::tls::Tls;
use crate::{api, files, voice};

/// How long open requests may take to finish after the shutdown signal. Together with closing
/// the database this stays within the 10 s the plan allows.
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(8);

/// How long QUIC gets to tell clients the connection is closing.
const QUIC_CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// Serves on `tcp` (and voice on `quic`, see [`voice::endpoint::bind`]) until `signal`
/// completes, then stops accepting connections, closes every gateway connection with 4100 and
/// every QUIC connection, waits up to [`DRAIN_TIMEOUT`] for open requests and closes the
/// database. Logs `Hermes is ready on <bind>:<port>` once serving.
pub async fn run(
    state: Arc<AppState>,
    tcp: TcpListener,
    tls: Tls,
    quic: Option<quinn::Endpoint>,
    signal: impl Future<Output = ()> + Send,
) -> io::Result<()> {
    let port = tcp.local_addr()?.port();
    let app = api::router(state.clone());
    let cleanup = tokio::spawn(files::cleanup_task(state.clone()));
    let refresh = tokio::spawn(crate::users::refresh_task(state.clone()));
    let calls = state
        .voice
        .take_after()
        .map(|rx| tokio::spawn(voice::calls::task(state.clone(), rx)));
    let voice_task = quic
        .clone()
        .map(|endpoint| tokio::spawn(voice::endpoint::serve(state.clone(), endpoint)));
    let reload = tls.reloader().cloned().map(|r| tokio::spawn(r.watch()));
    let (stop, stopped) = oneshot::channel::<()>();
    let graceful = {
        let state = state.clone();
        async move {
            let _ = stopped.await;
            state.gateway.shut_down();
        }
    };
    let mut server = tokio::spawn(async move { api::serve(tcp, &tls, app, graceful).await });
    tracing::info!("Hermes is ready on {}:{port}", state.config.network.bind);

    let result = tokio::select! {
        result = &mut server => joined(result),
        () = signal => {
            tracing::info!("shutting down");
            let _ = stop.send(());
            if let Some(quic) = &quic {
                quic.close(0u32.into(), b"server restarting");
            }
            match tokio::time::timeout(DRAIN_TIMEOUT, &mut server).await {
                Ok(result) => joined(result),
                Err(_) => {
                    tracing::warn!(
                        "requests still running after {}s; closing them",
                        DRAIN_TIMEOUT.as_secs()
                    );
                    server.abort();
                    Ok(())
                }
            }
        }
    };
    cleanup.abort();
    refresh.abort();
    if let Some(quic) = &quic {
        quic.close(0u32.into(), b"server restarting");
        let _ = tokio::time::timeout(QUIC_CLOSE_TIMEOUT, quic.wait_idle()).await;
    }
    for task in [calls, voice_task].into_iter().flatten() {
        task.abort();
    }
    if let Some(reload) = reload {
        reload.abort();
    }
    state.db.close().await;
    if result.is_ok() {
        tracing::info!("Hermes stopped");
    }
    result
}

fn joined(result: Result<io::Result<()>, JoinError>) -> io::Result<()> {
    result.unwrap_or_else(|err| Err(io::Error::other(format!("server task failed: {err}"))))
}

/// Completes on Ctrl+C / SIGINT, and on SIGTERM on unix.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!("cannot listen for Ctrl+C: {err}");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
            }
            Err(err) => {
                tracing::error!("cannot listen for SIGTERM: {err}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}
