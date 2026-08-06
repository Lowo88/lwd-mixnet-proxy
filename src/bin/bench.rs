//! Measures whether the probe and its retry are worth having.
//!
//! One trial is one call to the same `dial` the client half uses, against a running server half.
//! Each trial yields both headline numbers at once, from the same sample:
//!
//! - the **raw** rate, whether the first stream answered, which is what a wallet would see with no
//!   probe at all;
//! - the **wallet-visible** rate, whether any of them answered, which is what a wallet behind this
//!   proxy sees.
//!
//! Taking them from the same attempt rather than from alternating runs is what makes the comparison
//! survive the transport: its failure rate moved by an order of magnitude between sessions, which is
//! larger than any effect being measured here.
//!
//! Two things below the headline decide more than the headline does:
//!
//! - **Whether failures are independent.** Retrying multiplies only if they are. The per-round
//!   conditional rates answer this directly: if a second round fails as often as a first, retry
//!   scales and the required attempt count can be computed for any raw rate. If it fails more, no
//!   number of attempts is enough, and the headline rate of one good afternoon says nothing.
//! - **What establishing costs.** A failure is silent and therefore only discovered when the probe
//!   deadline expires, so retries buy reliability with seconds. A configuration can clear any
//!   failure-rate bar and still be one no wallet would tolerate.

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use lwd_mixnet_proxy::dial::{self, DialSettings, ProbeSettings, Round};
use nym_sdk::mixnet::{MixnetClient, Recipient};
use tokio::sync::Mutex;
use tokio::time::Instant;

#[derive(Parser)]
#[command(
    about = "Measure how often a probed stream answers, and what it costs",
    version
)]
struct Arguments {
    /// Nym address the serving half printed on startup.
    #[arg(long, env = "LWD_MIXNET_SERVER")]
    server: String,

    /// Fewer than a few hundred cannot separate the design from a quiet afternoon.
    #[arg(long, default_value_t = 300)]
    trials: usize,

    /// Streams one trial may open in total.
    #[arg(long, default_value = "4")]
    attempts: NonZeroU32,

    /// Streams opened at once. One measures sequential retry, where each failure costs a full
    /// timeout; more measures whether opening several at once buys latency without losing
    /// independence.
    #[arg(long, default_value = "1")]
    concurrency: NonZeroU32,

    #[arg(long, default_value_t = 10)]
    probe_timeout_secs: u64,

    #[arg(long)]
    reply_surbs: Option<u32>,
}

struct Trial {
    rounds: Vec<Round>,
    /// How long the whole trial took, retries included. This is what a wallet waits.
    total: Duration,
    /// How long the answering round took, or `None` if none did.
    established: Option<Duration>,
}

impl Trial {
    /// Whether the very first stream opened answered, which is the raw transport rate.
    fn first_stream_answered(&self) -> bool {
        self.rounds
            .first()
            .is_some_and(|round| round.answered && round.discarded.is_empty())
    }

    fn answered(&self) -> bool {
        self.established.is_some()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // The report on stdout is the result; the SDK's logging is diagnostics and belongs on stderr.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let arguments = Arguments::parse();

    let server: Recipient = arguments
        .server
        .parse()
        .context("parsing the server's Nym address")?;
    let settings = DialSettings {
        reply_surbs: arguments.reply_surbs,
        probe: Some(ProbeSettings {
            timeout: Duration::from_secs(arguments.probe_timeout_secs),
            attempts: arguments.attempts,
            concurrency: arguments.concurrency,
        }),
    };

    let client = MixnetClient::connect_new()
        .await
        .context("connecting to the mixnet")?;
    let client = Arc::new(Mutex::new(client));

    println!("dialling {server}");
    println!(
        "trials {} | attempts {} | {} at a time | probe timeout {}s | reply blocks {}\n",
        arguments.trials,
        arguments.attempts,
        arguments.concurrency,
        arguments.probe_timeout_secs,
        arguments
            .reply_surbs
            .map_or("SDK default".to_owned(), |surbs| surbs.to_string()),
    );

    let mut trials = Vec::with_capacity(arguments.trials);
    for number in 1..=arguments.trials {
        let trial = run_trial(&client, server, settings).await;
        println!(
            "{number:>5}  {:<9} {:>2} rounds  {:>2} discarded  {:>6} ms",
            if trial.answered() {
                "answered"
            } else {
                "GAVE UP"
            },
            trial.rounds.len(),
            trial
                .rounds
                .iter()
                .map(|round| round.discarded.len())
                .sum::<usize>(),
            trial.total.as_millis(),
        );
        trials.push(trial);
    }

    report(&trials, arguments.probe_timeout_secs);
    Ok(())
}

async fn run_trial(
    client: &Mutex<MixnetClient>,
    server: Recipient,
    settings: DialSettings,
) -> Trial {
    let started = Instant::now();
    match dial::dial(client, server, settings).await {
        // The stream is dropped rather than used: the server half connects to its upstream only
        // once a request arrives, so a measured stream never reaches the node.
        Ok(dialled) => Trial {
            rounds: dialled.rounds,
            total: started.elapsed(),
            established: Some(dialled.elapsed),
        },
        Err(gave_up) => Trial {
            rounds: gave_up.rounds,
            total: started.elapsed(),
            established: None,
        },
    }
}

fn report(trials: &[Trial], probe_timeout_secs: u64) {
    let total = trials.len();
    let raw_failures = trials
        .iter()
        .filter(|trial| !trial.first_stream_answered())
        .count();
    let visible_failures = trials.iter().filter(|trial| !trial.answered()).count();
    let streams: usize = trials
        .iter()
        .flat_map(|trial| trial.rounds.iter())
        .map(Round::opened)
        .sum();

    let raw_rate = ratio(raw_failures, total);
    let visible_rate = ratio(visible_failures, total);

    println!("\n--- headline ---");
    println!("trials                       {total}");
    println!(
        "raw failure rate             {:>6.2}%  ({raw_failures}/{total})   the first stream did not answer",
        100.0 * raw_rate
    );
    println!(
        "wallet-visible failure rate  {:>6.2}%  ({visible_failures}/{total})   no stream answered",
        100.0 * visible_rate
    );
    println!("streams opened               {streams}");
    if visible_rate > 0.0 {
        println!(
            "reduction factor             {:>6.1}x",
            raw_rate / visible_rate
        );
    } else if raw_failures > 0 {
        println!("reduction factor             >{total}x  (no visible failure in this run)");
    }

    if raw_failures * 10 < total {
        println!(
            "\nNOTE: a raw rate this low cannot separate the design from a quiet afternoon. \
             Treat the reduction above as unmeasured, not as passing."
        );
    }

    conditional_rates(trials);
    within_round(trials);
    latency(trials, probe_timeout_secs);

    if raw_rate > 0.0 && raw_rate < 1.0 {
        // Only meaningful if the conditional rates above show independence; printed alongside them
        // so the two are read together.
        let needed = (0.01f64.ln() / raw_rate.ln()).ceil() as u32;
        println!(
            "\nAt this raw rate, reaching 1% would take {needed} attempts if failures were independent."
        );
    }
}

/// How often round *j* failed, given that every round before it did.
///
/// Equal rates down the column mean failures are independent and retrying multiplies. A rising
/// column means a failure predicts the next one, and the arithmetic behind retrying does not hold.
fn conditional_rates(trials: &[Trial]) {
    println!("\n--- failure rate per round, conditional on every earlier round failing ---");
    println!("round  reached  failed    rate");

    let deepest = trials
        .iter()
        .map(|trial| trial.rounds.len())
        .max()
        .unwrap_or(0);
    for index in 0..deepest {
        let reached = trials
            .iter()
            .filter(|trial| trial.rounds.len() > index)
            .count();
        let failed = trials
            .iter()
            .filter_map(|trial| trial.rounds.get(index))
            .filter(|round| !round.answered)
            .count();
        println!(
            "{:>5}  {reached:>7}  {failed:>6}  {:>6.2}%",
            index + 1,
            100.0 * ratio(failed, reached)
        );
    }
}

/// How many streams failed inside one round, when a round opened more than one.
///
/// Streams opened together meet the same conditions, so this is the other half of the independence
/// question: whole rounds failing far more often than independent draws predict means the failure
/// belongs to a moment rather than to a stream.
fn within_round(trials: &[Trial]) {
    let mut spread: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for round in trials.iter().flat_map(|trial| trial.rounds.iter()) {
        if round.opened() > 1 {
            *spread
                .entry((round.opened(), round.discarded.len()))
                .or_default() += 1;
        }
    }
    if spread.is_empty() {
        return;
    }

    println!("\n--- streams that failed inside one round ---");
    println!("opened  failed  rounds");
    for ((opened, failed), count) in spread {
        println!("{opened:>6}  {failed:>6}  {count:>6}");
    }
}

fn latency(trials: &[Trial], probe_timeout_secs: u64) {
    let answering: Vec<u128> = trials
        .iter()
        .filter_map(|trial| trial.established)
        .map(|elapsed| elapsed.as_millis())
        .collect();
    let visible: Vec<u128> = trials
        .iter()
        .filter(|trial| trial.answered())
        .map(|trial| trial.total.as_millis())
        .collect();
    let discarded: Vec<u128> = trials
        .iter()
        .flat_map(|trial| trial.rounds.iter())
        .flat_map(|round| round.discarded.iter())
        .map(|stream| stream.elapsed.as_millis())
        .collect();

    println!("\n--- latency ---");
    print_percentiles("round that answered              ", &answering);
    print_percentiles("establishment seen by the wallet ", &visible);
    print_percentiles("time spent on a discarded stream ", &discarded);
    println!(
        "\nA discarded stream costs the whole probe timeout ({probe_timeout_secs}s), because the \
         failure is silent.\nThe second row is what a wallet waits before its first byte moves."
    );
}

fn print_percentiles(label: &str, samples: &[u128]) {
    if samples.is_empty() {
        println!("{label}  no samples");
        return;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let at = |fraction: f64| -> u128 {
        sorted[(((sorted.len() - 1) as f64) * fraction).round() as usize]
    };
    println!(
        "{label}  p50 {:>6} ms | p90 {:>6} ms | p99 {:>6} ms | max {:>6} ms",
        at(0.50),
        at(0.90),
        at(0.99),
        sorted[sorted.len() - 1]
    );
}

fn ratio(part: usize, whole: usize) -> f64 {
    part as f64 / whole.max(1) as f64
}
