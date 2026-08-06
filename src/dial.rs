//! Opening a stream that has been shown to work.
//!
//! Every attempt is a fresh stream, discarded whole when its probe goes unanswered. Nothing is
//! retried once the wallet's bytes are moving: resuming a conversation would mean rebuilding HTTP/2
//! state and replaying requests already in flight, and a request that was delivered twice is worse
//! than one that failed cleanly.
//!
//! Attempts are grouped into **rounds**, because how they are grouped decides what establishing a
//! connection costs. A failure is silent, so it is only ever discovered by the probe deadline
//! expiring: a round of one pays that deadline once per failure, in series, while a round of several
//! pays it once for the whole round and takes whichever stream answers first. The trade is reply
//! blocks, and that streams opened together meet the same conditions, so a bad moment can take all
//! of them at once.
//!
//! Both the rounds and the streams within them are reported to the caller rather than only logged.
//! The gap between how often a stream fails and how often the wallet notices is what justifies this
//! project, and whether retrying helps at all depends on failures being independent, which can only
//! be seen by counting them separately.

use std::num::NonZeroU32;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use nym_sdk::mixnet::{MixnetClient, MixnetStream, Recipient};
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::handshake::{self, HandshakeError};

/// How hard to insist on a stream that answers.
#[derive(Debug, Clone, Copy)]
pub struct ProbeSettings {
    /// How long one probe waits for its echo. Round trips measured seconds with a long tail, so a
    /// deadline tight enough to keep establishment quick also discards healthy streams, and the two
    /// distributions overlap: this is a trade, not a value to be tuned to a best.
    pub timeout: Duration,
    /// How many streams may be opened in total before giving up.
    pub attempts: NonZeroU32,
    /// How many are opened at once. One makes each failure cost a full deadline before the next
    /// attempt begins; more turns that sum into a minimum.
    pub concurrency: NonZeroU32,
}

/// How to reach the listening half.
#[derive(Debug, Clone, Copy)]
pub struct DialSettings {
    /// Reply blocks attached to each outbound message; `None` leaves the SDK default of 10. Raising
    /// it lowers the failure rate without ever reaching zero, and costs latency, so it is a knob
    /// and not a fix.
    pub reply_surbs: Option<u32>,
    /// `None` sends the header without waiting for it, which is the shape this takes if the
    /// transport ever stops losing first payloads.
    pub probe: Option<ProbeSettings>,
}

/// A stream that answered, and what it cost to get one.
pub struct Dialled {
    pub stream: MixnetStream,
    /// How long the answering attempt took, which is what the probe adds to establishing.
    pub elapsed: Duration,
    /// Every round run, the answering one last.
    pub rounds: Vec<Round>,
}

/// Every attempt failed.
#[derive(Debug, thiserror::Error)]
#[error("no stream answered after {} attempts", .rounds.iter().map(Round::opened).sum::<usize>())]
pub struct GaveUp {
    pub rounds: Vec<Round>,
}

/// One group of streams opened together.
#[derive(Debug, Default)]
pub struct Round {
    /// Streams thrown away, with why and how long each took.
    pub discarded: Vec<Discarded>,
    /// Whether one of this round's streams answered.
    pub answered: bool,
}

/// A stream that was opened and thrown away.
#[derive(Debug)]
pub struct Discarded {
    pub elapsed: Duration,
    pub error: DialError,
}

/// Why one attempt did not produce a usable stream.
#[derive(Debug, thiserror::Error)]
pub enum DialError {
    #[error("opening a mixnet stream: {0}")]
    Open(#[from] nym_sdk::Error),
    #[error("probing a fresh stream: {0}")]
    Probe(#[from] HandshakeError),
}

impl Round {
    /// How many streams this round opened.
    pub fn opened(&self) -> usize {
        self.discarded.len() + usize::from(self.answered)
    }
}

/// Open streams until one answers its probe, or until the attempts run out.
pub async fn dial(
    client: &Mutex<MixnetClient>,
    server: Recipient,
    settings: DialSettings,
) -> Result<Dialled, GaveUp> {
    let (attempts, concurrency) = match settings.probe {
        Some(probe) => (probe.attempts.get(), probe.concurrency.get()),
        None => (1, 1),
    };

    let mut rounds = Vec::new();
    let mut remaining = attempts;

    while remaining > 0 {
        let size = concurrency.min(remaining);
        remaining -= size;

        let (answered, round) = run_round(client, server, settings, size).await;
        rounds.push(round);

        if let Some((stream, elapsed)) = answered {
            return Ok(Dialled {
                stream,
                elapsed,
                rounds,
            });
        }
    }

    Err(GaveUp { rounds })
}

/// Open `size` streams together and keep the first one to answer.
///
/// The opens run before any probe does, deliberately: `open_stream` is documented as not cancel
/// safe, and cancelling one mid-flight leaves a stream registered with no owner. Probes are
/// cancelled freely, because by then the stream exists and dropping it deregisters it.
async fn run_round(
    client: &Mutex<MixnetClient>,
    server: Recipient,
    settings: DialSettings,
    size: u32,
) -> (Option<(MixnetStream, Duration)>, Round) {
    let started = Instant::now();
    let mut round = Round::default();
    let mut opened = Vec::with_capacity(size as usize);

    for _ in 0..size {
        // The lock is held for the open alone, which only queues a message. Concurrent callers
        // serialise on that and then run their probes in parallel.
        match client
            .lock()
            .await
            .open_stream(server, settings.reply_surbs)
            .await
        {
            Ok(stream) => opened.push(stream),
            Err(error) => round.discarded.push(Discarded {
                elapsed: started.elapsed(),
                error: error.into(),
            }),
        }
    }

    let Some(probe) = settings.probe else {
        // With no probe there is nothing to wait for, so the header goes out and the stream is used
        // as it is.
        let Some(mut stream) = opened.pop() else {
            return (None, round);
        };
        return match handshake::announce(&mut stream).await {
            Ok(()) => {
                round.answered = true;
                (Some((stream, started.elapsed())), round)
            }
            Err(error) => {
                round.discarded.push(Discarded {
                    elapsed: started.elapsed(),
                    error: error.into(),
                });
                (None, round)
            }
        };
    };

    let mut probing: FuturesUnordered<_> = opened
        .into_iter()
        .map(|mut stream| async move {
            match handshake::probe(&mut stream, probe.timeout).await {
                Ok(round_trip) => Ok((stream, round_trip)),
                Err(error) => Err(error),
            }
        })
        .collect();

    while let Some(result) = probing.next().await {
        match result {
            Ok((stream, round_trip)) => {
                tracing::debug!(?round_trip, "a stream answered its probe");
                round.answered = true;
                // Dropping the rest cancels their probes and deregisters their streams.
                return (Some((stream, started.elapsed())), round);
            }
            Err(error) => {
                tracing::debug!(%error, "discarding a stream that did not answer");
                round.discarded.push(Discarded {
                    elapsed: started.elapsed(),
                    error: error.into(),
                });
            }
        }
    }

    (None, round)
}

impl Dialled {
    /// How many streams were thrown away before one answered.
    pub fn discarded(&self) -> usize {
        self.rounds.iter().map(|round| round.discarded.len()).sum()
    }
}

impl GaveUp {
    /// What the last attempt failed with, which is the one worth reporting.
    pub fn last_error(&self) -> Option<&DialError> {
        self.rounds
            .last()?
            .discarded
            .last()
            .map(|attempt| &attempt.error)
    }

    /// How many streams were opened in total.
    pub fn attempts(&self) -> usize {
        self.rounds.iter().map(Round::opened).sum()
    }
}
