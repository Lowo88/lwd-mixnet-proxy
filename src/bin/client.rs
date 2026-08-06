//! Runs next to a wallet. Listens on a local TCP port and carries each connection over the mixnet.
//!
//! The wallet is pointed at the local port and needs no changes: what it gets is an ordinary TCP
//! connection that happens to be answered from far away, slowly.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use lwd_mixnet_proxy::dial::{self, DialSettings, ProbeSettings};
use lwd_mixnet_proxy::splice::{self, Watchdog};
use nym_sdk::mixnet::{MixnetClient, Recipient};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

#[derive(Parser)]
#[command(about = "Reach a light-client endpoint over the mixnet", version)]
struct Arguments {
    /// Nym address the serving half printed on startup.
    #[arg(long, env = "LWD_MIXNET_SERVER")]
    server: String,

    /// Local address the wallet connects to. Loopback by default: this port is an unauthenticated
    /// door to the upstream.
    #[arg(long, env = "LWD_MIXNET_BIND", default_value = "127.0.0.1:9068")]
    bind: String,

    /// How long a freshly opened stream has to answer its probe before it is discarded. Round trips
    /// measured seconds with a long tail, so a tighter deadline throws away healthy streams.
    #[arg(long, env = "LWD_MIXNET_PROBE_TIMEOUT_SECS", default_value_t = 10)]
    probe_timeout_secs: u64,

    /// How many streams may be opened for one wallet connection, the first included.
    #[arg(long, env = "LWD_MIXNET_PROBE_ATTEMPTS", default_value = "4")]
    probe_attempts: NonZeroU32,

    /// How many of those are opened at once. One retries in series, so each failure costs a whole
    /// probe timeout before the next attempt starts; more opens several and takes the first to
    /// answer, which costs reply blocks and exposes every stream in a round to the same moment.
    #[arg(long, env = "LWD_MIXNET_PROBE_CONCURRENCY", default_value = "2")]
    probe_concurrency: NonZeroU32,

    /// Hand the wallet the first stream that opens, without checking that it works. The probe
    /// exists because the transport loses first payloads silently; this is the switch for the day
    /// it stops.
    #[arg(long, env = "LWD_MIXNET_NO_PROBE")]
    no_probe: bool,

    /// Reply blocks attached to each outbound message; unset leaves the SDK default of 10. Raising
    /// it lowers the failure rate without reaching zero, and costs latency.
    #[arg(long, env = "LWD_MIXNET_REPLY_SURBS")]
    reply_surbs: Option<u32>,

    /// How long a connection may wait for an answer that never comes before it is closed. Closing
    /// it is what turns a silent hang into an error the wallet's gRPC library reconnects from.
    #[arg(long, env = "LWD_MIXNET_STALL_TIMEOUT_SECS", default_value_t = 60)]
    stall_timeout_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let arguments = Arguments::parse();

    let server: Recipient = arguments
        .server
        .parse()
        .context("parsing the server's Nym address")?;

    let dial_settings = DialSettings {
        reply_surbs: arguments.reply_surbs,
        probe: (!arguments.no_probe).then_some(ProbeSettings {
            timeout: Duration::from_secs(arguments.probe_timeout_secs),
            attempts: arguments.probe_attempts,
            concurrency: arguments.probe_concurrency,
        }),
    };
    let watchdog = Watchdog {
        stall: Some(Duration::from_secs(arguments.stall_timeout_secs)),
        idle: None,
    };

    // Deliberately ephemeral: a stable client identity is exactly what would let a server correlate
    // one wallet's requests across sessions.
    let client = MixnetClient::connect_new()
        .await
        .context("connecting to the mixnet")?;
    let client = Arc::new(Mutex::new(client));

    let listener = TcpListener::bind(&arguments.bind)
        .await
        .with_context(|| format!("binding {}", arguments.bind))?;
    tracing::info!(
        bind = %arguments.bind,
        probe = !arguments.no_probe,
        probe_attempts = arguments.probe_attempts,
        "carrying local connections over the mixnet"
    );

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (connection, wallet) = accepted.context("accepting a local connection")?;
                let client = Arc::clone(&client);
                tokio::spawn(async move {
                    carry(connection, &client, server, dial_settings, watchdog).await;
                    tracing::debug!(%wallet, "local connection done");
                });
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutting down");
                return Ok(());
            }
        }
    }
}

/// Carry one wallet connection over a stream that has been shown to work.
async fn carry(
    connection: TcpStream,
    client: &Mutex<MixnetClient>,
    server: Recipient,
    dial_settings: DialSettings,
    watchdog: Watchdog,
) {
    let dialled = match dial::dial(client, server, dial_settings).await {
        Ok(dialled) => dialled,
        Err(gave_up) => {
            // Dropping the connection is the point: the wallet sees a closed socket, which its gRPC
            // library knows how to retry, rather than a request that never returns.
            tracing::warn!(
                attempts = gave_up.attempts(),
                last_error = gave_up.last_error().map(|error| error.to_string()),
                "closing a local connection with no working stream to carry it"
            );
            return;
        }
    };

    if dialled.discarded() > 0 {
        tracing::info!(
            discarded = dialled.discarded(),
            rounds = dialled.rounds.len(),
            established = ?dialled.elapsed,
            "a stream answered after discarding streams that did not"
        );
    }

    let (transfer, ended) = splice::splice(connection, dialled.stream, watchdog).await;
    tracing::info!(
        sent = transfer.to_remote,
        received = transfer.from_remote,
        ?ended,
        "connection finished"
    );
}
