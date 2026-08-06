//! Measures whether the probe and its retry are worth having, and how they should be grouped.
//!
//! One trial is one call to the same `dial` the client half uses, against a running server half.
//! Each trial yields both headline numbers at once, from the same sample:
//!
//! - the **first-round failure rate**, how often the streams opened together all left their probes
//!   unanswered, which with rounds of one is the raw per-stream rate a wallet would meet with no
//!   probe at all;
//! - the **wallet-visible** rate, how often no stream in a trial answered at all.
//!
//! **Round sizes are interleaved trial by trial**, never run as separate sessions. The transport's
//! failure rate moved by an order of magnitude between one hour and the next, which is larger than
//! any effect being compared here, so two configurations measured an hour apart cannot be told apart
//! from the weather. Comparing crude and visible rates does not need this, because they come from
//! one attempt; comparing round sizes does, because they cannot.
//!
//! Two limits are worth knowing before reading a report:
//!
//! - **A per-stream rate is not measurable with rounds larger than one.** A round ends as soon as one
//!   stream answers, and the rest are cancelled seconds before their deadline would have expired, so
//!   the streams that were going to fail are dropped before they can. Only the round is measurable,
//!   which is why the round is what gets compared.
//! - **A stream the SDK refused to open is not a failure of the transport** and is counted apart. The
//!   local client was seen to degrade until it refused every open, and folding that in reads as the
//!   network failing and as failures becoming correlated, because a broken client fails every round
//!   of every trial. Any refusal marks the run invalid, and a client that stops opening altogether
//!   ends it.

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

/// Consecutive trials that open nothing before the run is abandoned. Enough to rule out a blip,
/// short enough not to spend the session on a dead client.
const REFUSAL_STREAK_LIMIT: usize = 5;

/// Every configuration under test, as the cross product of the axes given on the command line.
fn arms(arguments: &Arguments) -> Vec<Arm> {
    let gaps: Vec<Option<u64>> = match &arguments.gap_secs {
        Some(gaps) => gaps.iter().copied().map(Some).collect(),
        None => vec![None],
    };
    arguments
        .concurrency
        .iter()
        .flat_map(|concurrency| {
            gaps.iter().map(move |gap| Arm {
                concurrency: concurrency.get(),
                gap_secs: *gap,
            })
        })
        .collect()
}

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
    #[arg(long, default_value = "6")]
    attempts: NonZeroU32,

    /// Round sizes to compare, rotated one per trial. A single value measures one configuration;
    /// several interleave them, which is the only way to compare them on a transport that drifts.
    #[arg(long, value_delimiter = ',', default_value = "1,3")]
    concurrency: Vec<NonZeroU32>,

    /// Quiet gaps to leave before the measured stream is opened, in seconds, rotated one per trial.
    ///
    /// Set this to test whether the failure belongs to the stream or to the first real message after
    /// a pause. Each trial opens a warm-up stream and sends its header, waits the gap, and only then
    /// opens the stream it measures, so the clock starts at a known moment rather than at whatever
    /// the previous trial happened to end on. A rising failure rate across gaps says the pause is
    /// what matters; a flat one says the stream is.
    #[arg(long, value_delimiter = ',')]
    gap_secs: Option<Vec<u64>>,

    #[arg(long, default_value_t = 10)]
    probe_timeout_secs: u64,

    #[arg(long)]
    reply_surbs: Option<u32>,
}

/// One configuration under test. Trials rotate through these so every configuration meets the same
/// conditions, which on a transport that drifts by an order of magnitude is the only way two of them
/// can be compared at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Arm {
    concurrency: u32,
    /// Quiet gap left before the measured stream was opened, or `None` when not under test.
    gap_secs: Option<u64>,
}

impl std::fmt::Display for Arm {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.gap_secs {
            Some(gap) => write!(formatter, "rounds of {}, {gap}s gap", self.concurrency),
            None => write!(formatter, "rounds of {}", self.concurrency),
        }
    }
}

struct Trial {
    arm: Arm,
    rounds: Vec<Round>,
    /// How long the whole trial took, retries included. This is what a wallet waits.
    total: Duration,
    /// How long the answering round took, or `None` if none did.
    established: Option<Duration>,
}

impl Trial {
    fn concurrency(&self) -> u32 {
        self.arm.concurrency
    }

    fn answered(&self) -> bool {
        self.established.is_some()
    }

    /// Whether the streams opened together at the start all went unanswered.
    ///
    /// This is the comparable unit across round sizes: with rounds of one it is the raw per-stream
    /// rate, and with larger rounds it is how often a whole group came up empty.
    fn first_round_failed(&self) -> bool {
        self.rounds.first().is_some_and(|round| !round.answered)
    }

    fn opened(&self) -> usize {
        self.rounds.iter().map(|round| round.opened as usize).sum()
    }

    fn refused_opens(&self) -> usize {
        self.rounds.iter().map(Round::attempted).sum::<usize>() - self.opened()
    }

    fn unanswered(&self) -> usize {
        self.rounds.iter().map(Round::unanswered).sum()
    }

    fn cancelled(&self) -> usize {
        self.rounds.iter().map(Round::cancelled).sum()
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

    let client = MixnetClient::connect_new()
        .await
        .context("connecting to the mixnet")?;
    let client = Arc::new(Mutex::new(client));

    println!("dialling {server}");
    println!(
        "trials {} | attempts {} | round sizes {} interleaved | probe timeout {}s | reply blocks {}\n",
        arguments.trials,
        arguments.attempts,
        arguments
            .concurrency
            .iter()
            .map(|size| size.to_string())
            .collect::<Vec<_>>()
            .join(","),
        arguments.probe_timeout_secs,
        arguments
            .reply_surbs
            .map_or("SDK default".to_owned(), |surbs| surbs.to_string()),
    );

    let arms = arms(&arguments);
    let mut trials = Vec::with_capacity(arguments.trials);
    let mut refusing_streak = 0usize;

    for number in 1..=arguments.trials {
        let arm = arms[(number - 1) % arms.len()];
        let settings = DialSettings {
            reply_surbs: arguments.reply_surbs,
            probe: Some(ProbeSettings {
                timeout: Duration::from_secs(arguments.probe_timeout_secs),
                attempts: arguments.attempts,
                concurrency: NonZeroU32::new(arm.concurrency).unwrap_or(NonZeroU32::MIN),
            }),
        };

        // With a gap under test, the trial starts from a known moment rather than from wherever the
        // previous one happened to end, which differs by seconds depending on whether it was
        // answered or timed out.
        if let Some(gap) = arm.gap_secs {
            warm_up(&client, server, settings).await;
            tokio::time::sleep(Duration::from_secs(gap)).await;
        }

        let trial = run_trial(&client, server, settings, arm).await;
        // Single-word outcomes: a space here would shift every column after it for whoever parses
        // this later.
        println!(
            "{number:>5}  r{:<2}{:<5} {:<8} {:>2} rounds  {:>2} unanswered  {:>2} refused  {:>6} ms",
            trial.concurrency(),
            trial
                .arm
                .gap_secs
                .map_or(String::new(), |gap| format!("g{gap}")),
            if trial.answered() {
                "answered"
            } else {
                "gave-up"
            },
            trial.rounds.len(),
            trial.unanswered(),
            trial.refused_opens(),
            trial.total.as_millis(),
        );

        // A client whose gateway has gone stops opening streams at all, and everything measured
        // after that is the client rather than the transport. Stopping beats spending an hour
        // collecting numbers that have to be thrown away.
        refusing_streak = if trial.opened() == 0 {
            refusing_streak + 1
        } else {
            0
        };
        trials.push(trial);

        if refusing_streak >= REFUSAL_STREAK_LIMIT {
            println!(
                "\nSTOPPING after {number} trials: the last {refusing_streak} opened no stream at all. \
                 The local client is no longer able to send, so nothing further would measure the transport."
            );
            break;
        }
    }

    report(&trials, &arguments);
    Ok(())
}

/// Open a stream, send its header, and let it go, so a known instant of real traffic sits just
/// before the gap. Its outcome is deliberately not waited for or recorded: it exists to mark time,
/// not to be measured.
async fn warm_up(client: &Mutex<MixnetClient>, server: Recipient, settings: DialSettings) {
    let mut warming = settings;
    warming.probe = None;
    if let Ok(dialled) = dial::dial(client, server, warming).await {
        drop(dialled.stream);
    }
}

async fn run_trial(
    client: &Mutex<MixnetClient>,
    server: Recipient,
    settings: DialSettings,
    arm: Arm,
) -> Trial {
    let started = Instant::now();
    match dial::dial(client, server, settings).await {
        // The stream is dropped rather than used: the server half connects to its upstream only
        // once a request arrives, so a measured stream never reaches the node.
        Ok(dialled) => Trial {
            arm,
            rounds: dialled.rounds,
            total: started.elapsed(),
            established: Some(dialled.elapsed),
        },
        Err(gave_up) => Trial {
            arm,
            rounds: gave_up.rounds,
            total: started.elapsed(),
            established: None,
        },
    }
}

fn report(trials: &[Trial], arguments: &Arguments) {
    let refused: usize = trials.iter().map(Trial::refused_opens).sum();
    if refused > 0 {
        println!(
            "\nINVALID: {refused} opens were refused by the local client, which is a failure of this \
             machine rather than of the transport.\nEvery number below mixes the two. Restart the \
             client and measure again."
        );
    }

    let grouped: Vec<(Arm, Vec<&Trial>)> = arms(arguments)
        .into_iter()
        .map(|arm| {
            (
                arm,
                trials.iter().filter(|trial| trial.arm == arm).collect(),
            )
        })
        .collect();

    for (arm, group) in &grouped {
        report_arm(*arm, group, arguments.probe_timeout_secs);
    }
    if grouped.len() > 1 {
        compare(&grouped);
    }
}

fn report_arm(arm: Arm, group: &[&Trial], probe_timeout_secs: u64) {
    let size = arm.concurrency;
    let total = group.len();
    if total == 0 {
        return;
    }
    let first_round_failures = group
        .iter()
        .filter(|trial| trial.first_round_failed())
        .count();
    let visible_failures = group.iter().filter(|trial| !trial.answered()).count();
    let opened: usize = group.iter().map(|trial| trial.opened()).sum();
    let cancelled: usize = group.iter().map(|trial| trial.cancelled()).sum();

    let first_round_rate = ratio(first_round_failures, total);
    let visible_rate = ratio(visible_failures, total);

    println!(
        "\n=== {arm} ({}) ===",
        if size == 1 {
            "sequential retry"
        } else {
            "opened together"
        }
    );
    println!("trials                       {total}");
    println!(
        "first-round failure rate     {:>6.2}%  ({first_round_failures}/{total})   {}",
        100.0 * first_round_rate,
        if size == 1 {
            "the stream's probe went unanswered"
        } else {
            "no stream of the round answered"
        }
    );
    println!(
        "wallet-visible failure rate  {:>6.2}%  ({visible_failures}/{total})   no stream answered at all",
        100.0 * visible_rate
    );
    println!(
        "streams opened               {opened}   ({:.1} per trial, {cancelled} cancelled once another answered)",
        opened as f64 / total as f64
    );
    if visible_rate > 0.0 {
        println!(
            "reduction factor             {:>6.1}x",
            first_round_rate / visible_rate
        );
    } else if first_round_failures > 0 {
        println!("reduction factor             >{total}x  (no visible failure in this arm)");
    }
    if size > 1 {
        println!(
            "per-stream rate              not measurable with rounds of {size}: the losers are \
             cancelled seconds before their deadline"
        );
    }
    if first_round_failures * 10 < total {
        println!(
            "\nNOTE: a rate this low cannot separate the design from a quiet afternoon. \
             Treat any reduction above as unmeasured, not as passing."
        );
    }

    conditional_rates(group);
    within_round(group);
    latency(group, probe_timeout_secs);
}

/// Put the arms side by side, and check the assumption that makes larger rounds worth opening.
fn compare(arms: &[(Arm, Vec<&Trial>)]) {
    println!("\n=== comparison, interleaved in the same window ===");
    print!("{:28}", "");
    for (arm, _) in arms {
        print!("{:>20}", arm.to_string());
    }
    println!();

    let row = |label: &str, value: &dyn Fn(&[&Trial]) -> String| {
        print!("{label:28}");
        for (_, group) in arms {
            print!("{:>20}", value(group));
        }
        println!();
    };

    row("first-round failure rate", &|arm| {
        format!(
            "{:.2}%",
            100.0
                * ratio(
                    arm.iter().filter(|t| t.first_round_failed()).count(),
                    arm.len()
                )
        )
    });
    row("wallet-visible failure rate", &|arm| {
        format!(
            "{:.2}%",
            100.0 * ratio(arm.iter().filter(|t| !t.answered()).count(), arm.len())
        )
    });
    row("establishment p50", &|arm| {
        format!("{} ms", percentile(&establishment(arm), 0.50))
    });
    row("establishment p99", &|arm| {
        format!("{} ms", percentile(&establishment(arm), 0.99))
    });
    row("streams opened per trial", &|arm| {
        format!(
            "{:.1}",
            arm.iter().map(|t| t.opened()).sum::<usize>() as f64 / arm.len().max(1) as f64
        )
    });

    // With rounds of one measuring the per-stream rate, every larger round has a prediction to meet:
    // if simultaneous streams fail independently, a round of N fails at the per-stream rate to the
    // Nth. Observing far worse means the failure belongs to the moment rather than to the stream,
    // and opening more at once buys less than the arithmetic suggests.
    // Only meaningful when round size is the axis under test: a per-stream rate exists to be
    // squared only if some arm actually opened one stream at a time.
    if arms.iter().all(|(arm, _)| arm.concurrency == 1) {
        return;
    }
    let Some((_, sequential)) = arms.iter().find(|(arm, _)| arm.concurrency == 1) else {
        return;
    };
    let per_stream = ratio(
        sequential.iter().filter(|t| t.first_round_failed()).count(),
        sequential.len(),
    );
    if per_stream == 0.0 {
        return;
    }

    println!("\nAre simultaneous streams independent?");
    println!(
        "  per-stream failure rate, from rounds of 1   {:>7.2}%",
        100.0 * per_stream
    );
    for (arm, group) in arms.iter().filter(|(arm, _)| arm.concurrency > 1) {
        let predicted = per_stream.powi(arm.concurrency as i32);
        let observed = ratio(
            group.iter().filter(|t| t.first_round_failed()).count(),
            group.len(),
        );
        println!(
            "  a round of {} should then fail             {:>7.2}%,  observed {:>6.2}%",
            arm.concurrency,
            100.0 * predicted,
            100.0 * observed
        );
    }
}

fn establishment(arm: &[&Trial]) -> Vec<u128> {
    arm.iter()
        .filter(|trial| trial.answered())
        .map(|trial| trial.total.as_millis())
        .collect()
}

/// How often round *j* failed, given that every round before it did.
///
/// Equal rates down the column mean failures are independent and retrying multiplies. A rising
/// column means a failure predicts the next one, and the arithmetic behind retrying does not hold.
fn conditional_rates(arm: &[&Trial]) {
    let deepest = arm
        .iter()
        .map(|trial| trial.rounds.len())
        .max()
        .unwrap_or(0);
    if deepest < 2 {
        return;
    }

    println!("\nfailure rate per round, conditional on every earlier round failing");
    println!("round  reached  failed    rate");
    for index in 0..deepest {
        let reached = arm
            .iter()
            .filter(|trial| trial.rounds.len() > index)
            .count();
        let failed = arm
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

/// How many streams went unanswered inside one round, when a round opened more than one.
fn within_round(arm: &[&Trial]) {
    let mut spread: BTreeMap<(u32, usize), usize> = BTreeMap::new();
    for round in arm.iter().flat_map(|trial| trial.rounds.iter()) {
        if round.attempted() > 1 {
            *spread
                .entry((round.opened, round.unanswered()))
                .or_default() += 1;
        }
    }
    if spread.is_empty() {
        return;
    }

    println!("\nstreams that went unanswered inside one round");
    println!("opened  unanswered  rounds");
    for ((opened, unanswered), count) in spread {
        println!("{opened:>6}  {unanswered:>10}  {count:>6}");
    }
}

fn latency(arm: &[&Trial], probe_timeout_secs: u64) {
    let answering: Vec<u128> = arm
        .iter()
        .filter_map(|trial| trial.established)
        .map(|elapsed| elapsed.as_millis())
        .collect();
    let discarded: Vec<u128> = arm
        .iter()
        .flat_map(|trial| trial.rounds.iter())
        .flat_map(|round| round.discarded.iter())
        .map(|stream| stream.elapsed.as_millis())
        .collect();

    println!("\nlatency");
    print_percentiles("round that answered              ", &answering);
    print_percentiles("establishment seen by the wallet ", &establishment(arm));
    print_percentiles("time spent on a discarded stream ", &discarded);
    println!(
        "a discarded stream costs the whole probe timeout ({probe_timeout_secs}s), because the \
         failure is silent"
    );
}

fn print_percentiles(label: &str, samples: &[u128]) {
    if samples.is_empty() {
        println!("{label}  no samples");
        return;
    }
    println!(
        "{label}  p50 {:>6} ms | p90 {:>6} ms | p99 {:>6} ms | max {:>6} ms",
        percentile(samples, 0.50),
        percentile(samples, 0.90),
        percentile(samples, 0.99),
        percentile(samples, 1.0),
    );
}

fn percentile(samples: &[u128], fraction: f64) -> u128 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted[(((sorted.len() - 1) as f64) * fraction).round() as usize]
}

fn ratio(part: usize, whole: usize) -> f64 {
    part as f64 / whole.max(1) as f64
}
