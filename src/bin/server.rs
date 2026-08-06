//! Runs next to a lightwalletd. Accepts mixnet streams and splices each one to an upstream.
//!
//! The upstream is whatever the operator points it at, so this serves any implementation of the
//! light-client protocol without knowing which one it is.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use lwd_mixnet_proxy::handshake;
use lwd_mixnet_proxy::splice::{self, Watchdog};
use nym_sdk::mixnet::{MixnetClient, MixnetClientBuilder, MixnetStream, StoragePaths};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Big enough for a gRPC client's opening burst, so the first write upstream is a single one.
const FIRST_CHUNK: usize = 8 * 1024;

#[derive(Parser)]
#[command(
    about = "Serve an upstream light-client endpoint over the mixnet",
    version
)]
struct Arguments {
    /// Endpoint every accepted stream is spliced to.
    #[arg(long, env = "LWD_MIXNET_UPSTREAM", default_value = "127.0.0.1:9067")]
    upstream: String,

    /// Directory holding the client identity. Without it the Nym address is ephemeral and rotates
    /// on every restart, which makes this half unreachable by anyone who wrote the address down.
    ///
    /// The directory holds private keys: losing it changes the address, copying it allows
    /// impersonation.
    #[arg(long, env = "LWD_MIXNET_STATE_DIR")]
    state_dir: Option<PathBuf>,

    /// How long an accepted stream has to say who it is. This is what a stream whose payload never
    /// arrived costs: without it, the SDK holds one for half an hour.
    #[arg(long, env = "LWD_MIXNET_HANDSHAKE_TIMEOUT_SECS", default_value_t = 30)]
    handshake_timeout_secs: u64,

    /// How long a stream may go without moving a byte before it is let go. The transport carries no
    /// close, so a dialler that walked away is only ever noticed by this timer.
    #[arg(long, env = "LWD_MIXNET_IDLE_TIMEOUT_SECS", default_value_t = 600)]
    idle_timeout_secs: u64,
}

#[derive(Clone)]
struct Settings {
    upstream: String,
    handshake_timeout: Duration,
    idle_timeout: Duration,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Logs go to stderr so stdout carries nothing but the address line below.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let arguments = Arguments::parse();

    let settings = Settings {
        upstream: arguments.upstream.clone(),
        handshake_timeout: Duration::from_secs(arguments.handshake_timeout_secs),
        idle_timeout: Duration::from_secs(arguments.idle_timeout_secs),
    };

    let mut client = connect(&arguments, settings.idle_timeout).await?;

    // Printed rather than logged so it survives whatever the log filter is set to: an operator
    // cannot configure the dialling half without it.
    let address = *client.nym_address();
    println!("NYM_ADDRESS={address}");

    let mut listener = client.listener().context("taking the stream listener")?;
    tracing::info!(
        %address,
        upstream = %settings.upstream,
        handshake_timeout_secs = arguments.handshake_timeout_secs,
        idle_timeout_secs = arguments.idle_timeout_secs,
        "accepting mixnet streams"
    );

    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Some(stream) => {
                    let settings = settings.clone();
                    tokio::spawn(serve(stream, settings));
                }
                None => {
                    tracing::warn!("the mixnet listener closed");
                    return Ok(());
                }
            },
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutting down");
                return Ok(());
            }
        }
    }
}

async fn connect(arguments: &Arguments, idle_timeout: Duration) -> Result<MixnetClient> {
    match &arguments.state_dir {
        Some(directory) => {
            let paths = StoragePaths::new_from_dir(directory)
                .context("preparing the client state directory")?;
            MixnetClientBuilder::new_with_default_storage(paths)
                .await
                .context("building a client with persistent storage")?
                // A registered gateway that is temporarily unbonded should delay startup rather
                // than fail it: registration itself was observed to fail on 2 of 15 attempts.
                .with_wait_for_gateway(true)
                .with_stream_idle_timeout(idle_timeout)
                .build()
                .context("assembling the client")?
                .connect_to_mixnet()
                .await
                .context("connecting to the mixnet")
        }
        None => MixnetClientBuilder::new_ephemeral()
            .with_stream_idle_timeout(idle_timeout)
            .build()
            .context("assembling an ephemeral client")?
            .connect_to_mixnet()
            .await
            .context("connecting an ephemeral client to the mixnet"),
    }
}

/// Splice one accepted stream to the upstream, once it has proven to be one of ours.
async fn serve(mut stream: MixnetStream, settings: Settings) {
    let stream_id = stream.id();

    if let Err(error) = handshake::accept(&mut stream, settings.handshake_timeout).await {
        tracing::debug!(%stream_id, %error, "dropping a stream that never introduced itself");
        return;
    }

    // The upstream is not touched until the dialler sends something to send it. A stream that was
    // only probed, or one whose dialler discarded it, therefore never becomes a connection to the
    // node: it waits here and is let go on the idle deadline.
    let mut opening = vec![0u8; FIRST_CHUNK];
    let read = match tokio::time::timeout(settings.idle_timeout, stream.read(&mut opening)).await {
        Ok(Ok(0)) | Err(_) => {
            tracing::debug!(%stream_id, "letting go of a stream that carried no request");
            return;
        }
        Ok(Ok(read)) => read,
        Ok(Err(error)) => {
            tracing::debug!(%stream_id, %error, "reading the first request failed");
            return;
        }
    };

    let mut upstream = match TcpStream::connect(&settings.upstream).await {
        Ok(connection) => connection,
        Err(error) => {
            tracing::error!(%stream_id, %error, upstream = %settings.upstream, "upstream unreachable");
            return;
        }
    };
    if let Err(error) = upstream.write_all(&opening[..read]).await {
        tracing::warn!(%stream_id, %error, "writing the first request upstream failed");
        return;
    }

    let watchdog = Watchdog {
        // A completed response leaves the connection legitimately quiet, so a stalled-request
        // deadline would fire on healthy streams here. Only the reaper applies.
        stall: None,
        idle: Some(settings.idle_timeout),
    };
    let (transfer, ended) = splice::splice(upstream, stream, watchdog).await;
    tracing::info!(
        %stream_id,
        from_client = transfer.from_remote + read as u64,
        to_client = transfer.to_remote,
        ?ended,
        "stream finished"
    );
}
