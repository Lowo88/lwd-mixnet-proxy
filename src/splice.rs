//! Copying bytes between a local connection and a mixnet stream, under a deadline.
//!
//! The transport offers no way to signal a close: dropping a stream tells the far side nothing, so
//! an end that walks away leaves the other holding a conversation that will never continue. Neither
//! side can wait for an error that is never coming, which makes the timers here the only thing
//! ending such a stream.
//!
//! Two of them, because a connection can be dead in two different ways:
//!
//! - **stalled**: the far side was written to and has answered nothing since. This is a request
//!   without a response, and it is what the dialling half watches for: closing the local connection
//!   turns an invisible hang into an ordinary error the wallet's own gRPC library reconnects from.
//!   The clock runs from the oldest unanswered write, so a wallet that keeps talking into a dead
//!   stream — keepalive pings, say — cannot postpone the discovery.
//! - **idle**: nothing has moved in either direction. An idle connection is legitimate, so this is a
//!   reaper rather than a failure detector: it is what the listening half uses to let go of streams
//!   whose dialler is gone.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;

/// Large enough that a stream of compact blocks is not chopped into needless mixnet messages: each
/// write becomes one message carrying its own reply blocks, so small writes are expensive here in a
/// way they are not on a socket.
const COPY_BUFFER: usize = 8 * 1024;

/// How long a splice may go without the far side answering, and without moving at all.
#[derive(Debug, Clone, Copy, Default)]
pub struct Watchdog {
    /// Give up when the far side was sent bytes and has answered nothing for this long, counted
    /// from the first write of the unanswered run rather than the latest one.
    pub stall: Option<Duration>,
    /// Give up when nothing has moved in either direction for this long.
    pub idle: Option<Duration>,
}

/// How a splice ended.
#[derive(Debug)]
pub enum Ended {
    /// One side reached end of stream, which is the ordinary close.
    Closed,
    /// The far side was written to and answered nothing.
    Stalled(Duration),
    /// Nothing moved in either direction.
    Idle(Duration),
    /// A copy failed, so one side is gone or the transport broke.
    Failed(io::Error),
}

/// How many bytes moved, per direction.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Transfer {
    pub to_remote: u64,
    pub from_remote: u64,
}

/// Copy bytes both ways until one direction ends or the watchdog trips.
///
/// Returning rather than erroring is deliberate: every outcome here is a way a connection ends
/// normally, and the caller wants to record which one it was.
pub async fn splice<L, R>(local: L, remote: R, watchdog: Watchdog) -> (Transfer, Ended)
where
    L: AsyncRead + AsyncWrite + Unpin,
    R: AsyncRead + AsyncWrite + Unpin,
{
    let started = Instant::now();
    let activity = Activity::default();

    let (mut local_reader, mut local_writer) = tokio::io::split(local);
    let (mut remote_reader, mut remote_writer) = tokio::io::split(remote);

    // Neither pump ever shuts its writer down. A mixnet stream's shutdown deregisters the whole
    // stream rather than half-closing it, which would silently kill the other direction.
    let outbound = pump(
        &mut local_reader,
        &mut remote_writer,
        &activity.to_remote,
        started,
    );
    let inbound = pump(
        &mut remote_reader,
        &mut local_writer,
        &activity.from_remote,
        started,
    );
    tokio::pin!(outbound, inbound);

    let period = watchdog.check_period();
    let mut ticker = tokio::time::interval_at(Instant::now() + period, period);
    let mut stall_clock = StallClock::default();

    loop {
        tokio::select! {
            result = &mut outbound => return (activity.transfer(), ended(result)),
            result = &mut inbound => return (activity.transfer(), ended(result)),
            _ = ticker.tick() => {
                if let Some(reason) = watchdog.tripped(&mut stall_clock, &activity, started) {
                    return (activity.transfer(), reason);
                }
            }
        }
    }
}

/// When the current unanswered period began, if one is running.
#[derive(Default)]
struct StallClock {
    since_millis: Option<u64>,
}

impl Watchdog {
    fn tripped(
        &self,
        clock: &mut StallClock,
        activity: &Activity,
        started: Instant,
    ) -> Option<Ended> {
        // Read before the timestamps, so bytes landing in between make the subtractions below
        // saturate to zero rather than wrap: a deadline that has not been reached must never look
        // like one that has.
        let now = millis_since(started);
        let wrote = activity.to_remote.moved_at();
        let heard = activity.from_remote.moved_at();

        // A request is outstanding when the far side was written to and has said nothing since.
        match wrote.filter(|wrote| heard.is_none_or(|heard| heard < *wrote)) {
            Some(wrote) => {
                // Writes that follow do not move the start: letting them would hand a chatty wallet
                // the power to postpone the deadline forever. Answers do move it, because whatever
                // came back settled everything written before it, and a connection carrying a
                // stream of replies is answering even while its newest write waits its turn.
                let since = clock.since_millis.get_or_insert(wrote);
                *since = (*since).max(heard.unwrap_or(0));
                if let Some(stall) = self.stall
                    && now.saturating_sub(*since) >= as_millis(stall)
                {
                    return Some(Ended::Stalled(stall));
                }
            }
            None => clock.since_millis = None,
        }
        // Nothing having moved at all counts from the beginning of the splice.
        if let Some(idle) = self.idle
            && now.saturating_sub(wrote.into_iter().chain(heard).max().unwrap_or(0))
                >= as_millis(idle)
        {
            return Some(Ended::Idle(idle));
        }
        None
    }

    /// Checking four times per deadline keeps the overshoot under a quarter of it. With no deadline
    /// set the loop still ticks, rarely, and finds nothing to do.
    fn check_period(&self) -> Duration {
        const IDLE_PERIOD: Duration = Duration::from_secs(3600);
        const FLOOR: Duration = Duration::from_millis(100);
        const CEILING: Duration = Duration::from_secs(5);

        match (self.stall, self.idle) {
            (None, None) => IDLE_PERIOD,
            (stall, idle) => (stall.into_iter().chain(idle).min().unwrap_or(IDLE_PERIOD) / 4)
                .clamp(FLOOR, CEILING),
        }
    }
}

/// One direction's byte count and the moment it last moved, in milliseconds since the splice began.
#[derive(Default)]
struct Direction {
    bytes: AtomicU64,
    last_at_millis: AtomicU64,
}

impl Direction {
    /// When this direction last moved, or `None` if it never has.
    ///
    /// The byte count is what distinguishes the two, because a millisecond count cannot: bytes that
    /// move in the first millisecond are stamped zero, and reading that as "never" leaves a request
    /// sent immediately after opening, which is the ordinary case, unable to be seen as outstanding.
    fn moved_at(&self) -> Option<u64> {
        (self.bytes.load(Ordering::Relaxed) > 0)
            .then(|| self.last_at_millis.load(Ordering::Relaxed))
    }

    fn record(&self, moved: usize, started: Instant) {
        // Stamped before it is counted, so a non-zero count always has a timestamp behind it.
        self.last_at_millis
            .store(millis_since(started), Ordering::Relaxed);
        self.bytes.fetch_add(moved as u64, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct Activity {
    to_remote: Direction,
    from_remote: Direction,
}

impl Activity {
    fn transfer(&self) -> Transfer {
        Transfer {
            to_remote: self.to_remote.bytes.load(Ordering::Relaxed),
            from_remote: self.from_remote.bytes.load(Ordering::Relaxed),
        }
    }
}

async fn pump<R, W>(
    reader: &mut R,
    writer: &mut W,
    direction: &Direction,
    started: Instant,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; COPY_BUFFER];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        writer.write_all(&buffer[..read]).await?;
        writer.flush().await?;
        direction.record(read, started);
    }
}

fn ended(result: io::Result<()>) -> Ended {
    match result {
        Ok(()) => Ended::Closed,
        Err(error) => Ended::Failed(error),
    }
}

fn millis_since(started: Instant) -> u64 {
    as_millis(started.elapsed())
}

fn as_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMMEDIATE: Duration = Duration::from_millis(1);

    /// A peer that accepts everything and answers nothing, which is what a lost stream looks like.
    fn deaf_peer() -> tokio::io::DuplexStream {
        let (near, far) = tokio::io::duplex(COPY_BUFFER);
        tokio::spawn(async move {
            let mut far = far;
            let mut sink = Vec::new();
            let _ = far.read_to_end(&mut sink).await;
        });
        near
    }

    /// Splice, failing the test if a deadline that should fire never does.
    ///
    /// A watchdog that never trips spins forever rather than hanging: with time paused, the runtime
    /// keeps advancing the clock to the next tick and no test harness interrupts that.
    async fn splice_expecting_a_deadline<L, R>(
        local: L,
        remote: R,
        watchdog: Watchdog,
    ) -> (Transfer, Ended)
    where
        L: AsyncRead + AsyncWrite + Unpin,
        R: AsyncRead + AsyncWrite + Unpin,
    {
        tokio::time::timeout(Duration::from_secs(3600), splice(local, remote, watchdog))
            .await
            .expect("a deadline should have fired")
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_the_far_side_never_answers_trips_the_stall_deadline() {
        let (mut wallet, local) = tokio::io::duplex(COPY_BUFFER);
        wallet.write_all(b"a request").await.unwrap();

        let watchdog = Watchdog {
            stall: Some(Duration::from_secs(30)),
            idle: None,
        };
        let (transfer, ended) = splice_expecting_a_deadline(local, deaf_peer(), watchdog).await;

        assert!(matches!(ended, Ended::Stalled(_)) && transfer.to_remote == 9);
    }

    #[tokio::test(start_paused = true)]
    async fn a_wallet_that_keeps_writing_does_not_postpone_the_stall_deadline() {
        let (mut wallet, local) = tokio::io::duplex(COPY_BUFFER);
        tokio::spawn(async move {
            // Keepalive pings on a dead stream: each lands well inside the deadline, so a clock
            // restarted by every write would never reach it.
            loop {
                if wallet.write_all(b"ping").await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });

        let watchdog = Watchdog {
            stall: Some(Duration::from_secs(30)),
            idle: None,
        };
        let started = Instant::now();
        let (_, ended) = splice_expecting_a_deadline(local, deaf_peer(), watchdog).await;

        assert!(
            matches!(ended, Ended::Stalled(_)) && started.elapsed() < Duration::from_secs(60),
            "the deadline should fire near 30 s regardless of further writes: {ended:?} after {:?}",
            started.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_clears_the_stall_deadline() {
        let (mut wallet, local) = tokio::io::duplex(COPY_BUFFER);
        let (mut server, remote) = tokio::io::duplex(COPY_BUFFER);
        wallet.write_all(b"a request").await.unwrap();
        tokio::spawn(async move {
            let mut request = [0u8; 9];
            server.read_exact(&mut request).await.unwrap();
            server.write_all(b"an answer").await.unwrap();
            // Holding the connection open afterwards is an ordinary idle gRPC connection.
            std::future::pending::<()>().await;
        });

        let watchdog = Watchdog {
            stall: Some(Duration::from_secs(30)),
            idle: Some(Duration::from_secs(600)),
        };
        let (transfer, ended) = splice_expecting_a_deadline(local, remote, watchdog).await;

        assert!(
            matches!(ended, Ended::Idle(_)) && transfer.from_remote == 9,
            "an answered request should idle out, not stall: {ended:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_second_request_after_an_answer_gets_a_fresh_stall_window() {
        let (mut wallet, local) = tokio::io::duplex(COPY_BUFFER);
        let (mut server, remote) = tokio::io::duplex(COPY_BUFFER);
        let driver = tokio::spawn(async move {
            wallet.write_all(b"first").await.unwrap();
            let mut answer = [0u8; 6];
            wallet.read_exact(&mut answer).await.unwrap();
            tokio::time::sleep(Duration::from_secs(100)).await;
            wallet.write_all(b"second").await.unwrap();
            std::future::pending::<()>().await;
        });
        tokio::spawn(async move {
            let mut request = [0u8; 5];
            server.read_exact(&mut request).await.unwrap();
            server.write_all(b"answer").await.unwrap();
            let mut second = [0u8; 6];
            let _ = server.read_exact(&mut second).await;
            std::future::pending::<()>().await;
        });

        let watchdog = Watchdog {
            stall: Some(Duration::from_secs(30)),
            idle: None,
        };
        let started = Instant::now();
        let (_, ended) = splice_expecting_a_deadline(local, remote, watchdog).await;
        driver.abort();

        assert!(
            matches!(ended, Ended::Stalled(_))
                && started.elapsed() > Duration::from_secs(100)
                && started.elapsed() < Duration::from_secs(160),
            "the second request should stall ~30 s after it was sent: {ended:?} after {:?}",
            started.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_connection_nothing_ever_moves_on_trips_the_idle_deadline() {
        let (_wallet, local) = tokio::io::duplex(COPY_BUFFER);
        let watchdog = Watchdog {
            stall: None,
            idle: Some(Duration::from_secs(600)),
        };
        let (transfer, ended) = splice_expecting_a_deadline(local, deaf_peer(), watchdog).await;

        assert!(matches!(ended, Ended::Idle(_)) && transfer == Transfer::default());
    }

    #[tokio::test]
    async fn bytes_cross_in_both_directions() {
        let (mut wallet, local) = tokio::io::duplex(COPY_BUFFER);
        let (mut server, remote) = tokio::io::duplex(COPY_BUFFER);
        wallet.write_all(b"question").await.unwrap();
        tokio::spawn(async move {
            let mut request = [0u8; 8];
            server.read_exact(&mut request).await.unwrap();
            server.write_all(b"answer").await.unwrap();
        });

        let spliced = tokio::spawn(splice(local, remote, Watchdog::default()));
        let mut answer = [0u8; 6];
        wallet.read_exact(&mut answer).await.unwrap();
        drop(wallet);

        let (transfer, _) = spliced.await.unwrap();
        assert_eq!(
            transfer,
            Transfer {
                to_remote: 8,
                from_remote: 6
            }
        );
    }

    #[tokio::test]
    async fn the_wallet_hanging_up_ends_the_splice() {
        let (wallet, local) = tokio::io::duplex(COPY_BUFFER);
        drop(wallet);
        let (_, ended) = splice(local, deaf_peer(), Watchdog::default()).await;
        assert!(matches!(ended, Ended::Closed));
    }

    #[tokio::test(start_paused = true)]
    async fn the_stall_deadline_is_checked_well_inside_its_own_window() {
        let (mut wallet, local) = tokio::io::duplex(COPY_BUFFER);
        wallet.write_all(b"a request").await.unwrap();
        let watchdog = Watchdog {
            stall: Some(Duration::from_secs(30)),
            idle: None,
        };

        let started = Instant::now();
        let _ = splice_expecting_a_deadline(local, deaf_peer(), watchdog).await;

        assert!(started.elapsed() < Duration::from_secs(40));
    }

    #[test]
    fn a_watchdog_with_no_deadline_still_ticks() {
        assert!(Watchdog::default().check_period() > IMMEDIATE);
    }

    #[tokio::test(start_paused = true)]
    async fn a_long_answer_delivered_in_instalments_is_not_mistaken_for_a_stall() {
        let (mut wallet, local) = tokio::io::duplex(COPY_BUFFER);
        let (mut server, remote) = tokio::io::duplex(COPY_BUFFER);

        // One request, then the wallet only acknowledges what arrives, the way flow control does,
        // while the far side answers in instalments for four times the deadline. Every ack lands
        // after the instalment that prompted it, so the newest write is almost always unanswered.
        let acknowledging = tokio::spawn(async move {
            wallet.write_all(b"request").await.unwrap();
            let mut instalment = [0u8; 8];
            while wallet.read_exact(&mut instalment).await.is_ok() {
                if wallet.write_all(b"ack").await.is_err() {
                    return;
                }
            }
        });
        tokio::spawn(async move {
            let mut request = [0u8; 7];
            server.read_exact(&mut request).await.unwrap();
            for _ in 0..24 {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if server.write_all(b"instalme").await.is_err() {
                    return;
                }
            }
            // Dropping the stream here is the ordinary end of a streaming call.
        });

        let watchdog = Watchdog {
            stall: Some(Duration::from_secs(30)),
            idle: None,
        };
        let (transfer, ended) = splice(local, remote, watchdog).await;
        acknowledging.abort();

        assert!(
            matches!(ended, Ended::Closed) && transfer.from_remote == 24 * 8,
            "a stream of answers must not read as a stall: {ended:?}, {transfer:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_reply_stream_interleaved_with_writes_never_adds_up_to_a_stall() {
        let started = Instant::now();
        let activity = Activity::default();
        let watchdog = Watchdog {
            stall: Some(Duration::from_secs(30)),
            idle: None,
        };
        let mut clock = StallClock::default();

        // What a bulk download looks like from here: blocks arrive, the wallet answers with flow
        // control, and every observation lands while that newest write is still unanswered. The
        // far side is plainly talking, so none of it may accumulate toward the deadline.
        let mut tripped = None;
        for _ in 0..12 {
            activity.from_remote.record(1, started);
            tokio::time::advance(Duration::from_secs(5)).await;
            activity.to_remote.record(1, started);
            tripped = tripped.or(watchdog.tripped(&mut clock, &activity, started));
            tokio::time::advance(Duration::from_secs(5)).await;
        }

        assert!(tripped.is_none(), "{tripped:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn bytes_landing_while_the_watchdog_reads_the_clock_do_not_trip_it() {
        let started = Instant::now();
        let activity = Activity::default();
        // What a `record()` between reading the clock and loading the timestamps leaves behind: a
        // stamp ahead of the watchdog's own `now`.
        activity
            .to_remote
            .record(1, started - Duration::from_secs(1));

        let watchdog = Watchdog {
            stall: Some(Duration::from_secs(30)),
            idle: Some(Duration::from_secs(600)),
        };

        assert!(
            watchdog
                .tripped(&mut StallClock::default(), &activity, started)
                .is_none()
        );
    }
}
