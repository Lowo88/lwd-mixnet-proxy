# Does probing and retrying work, and how should attempts be grouped?

Date: 2026-08-06. Evidence for [ADR 0003](../decisions/0003-probe-every-stream-before-the-wallet-uses-it.md)
and [ADR 0005](../decisions/0005-what-the-measurement-has-to-show.md).

## Method

`lwd-mixnet-bench` drives the same `dial` the client half calls, against a running `lwd-mixnet-server`
on the same machine, so the only thing between the two ends is the mixnet. Both binaries were release
builds: in a debug build the SDK's Sphinx cryptography is unoptimised, which would distort the
latencies the whole result turns on.

One trial is one dial. Round sizes are **interleaved trial by trial** rather than run as separate
sessions, and per-round conditional rates are recorded so that whether retrying multiplies is measured,
not assumed. The reasoning behind both is in ADR 0005.

Four runs were made. The two that were thrown out are reported because what invalidated them is a
result in itself.

## Run 3: rounds of 1 against rounds of 3, interleaved

300 trials, 150 per arm, 6 attempts allowed, 10 s probe deadline, SDK default reply blocks. No opens
were refused, so the local client was healthy throughout.

| | rounds of 1 | rounds of 3 |
|---|---|---|
| first-round failure rate | **34.67%** (52/150) | **0.67%** (1/150) |
| wallet-visible failure rate | 0.00% (0/150) | 0.00% (0/150) |
| establishment p50 | 1,323 ms | 1,240 ms |
| establishment p90 | 11,409 ms | 1,419 ms |
| establishment p99 | **31,276 ms** | **6,340 ms** |
| streams opened per trial | 1.4 | 3.0 |

**Failures between rounds are independent.** In the sequential arm, the failure rate conditional on
every earlier round having failed does not rise: 34.67%, then 19.23%, then 20.00%. A stream that
fails says nothing about the next one, which is what makes retrying multiply at all.

**Both arms reduce a one-in-three failure to nothing observable**, 0 of 150 in each. The failure rate
is not what separates them.

**Latency is.** A silent failure is only discovered when the deadline expires, so in the sequential
arm a third of connections spend a full 10 s before their retry even begins, and the whole tail moves
with them: p90 of 11.4 s and p99 of 31.3 s, against 1.4 s and 6.3 s for rounds of three. Rounds of
three cost 3.0 streams per connection instead of 1.4, and the reply-block budget that goes with them.

### Simultaneous streams did better than independence predicts

At a 34.67% per-stream rate, a round of three should fail 4.17% of the time. It failed 0.67%, one
round in 150, where about six were expected.

The explanation that suggested itself was that the per-stream rate might belong to **the first message
after a pause** rather than to the stream: in the sequential arm every trial opens its first stream
after a wait, while in the hedged arm only one of the three does. That was tested directly and does
not hold, so the discrepancy remains unexplained and is most likely chance. One observed event against
six expected is a p of about 1.4%, which is not much against a run in which several numbers were being
watched at once.

## Run 4: does traffic have to be warm?

If the failure belonged to the first real message after a quiet period, keeping traffic warm would
reduce it, and that would matter well beyond this project.

Each trial opened a warm-up stream, sent its header without waiting for anything back, waited a gap,
and only then opened the stream it measured, with no retry so that every trial is one clean sample.
Gaps of 0, 2, 8 and 20 seconds were rotated one per trial. Starting the clock from the warm-up rather
than from the previous trial matters: a previous trial ends either on an answer or on a deadline,
seconds apart, which would smear the variable being set.

| gap before the measured stream | failure rate |
|---|---|
| 0 s | 31.4% |
| 2 s | 23.5% |
| 8 s | 35.3% |
| 20 s | 29.4% |

137 clean trials, about 34 per arm, at a 29.9% overall rate. One standard error per arm is about 7.9
points, so all four are one number. **The gap makes no difference**, and the ordering does not even
run the predicted way: the warmest arm failed slightly more than the coldest.

A second check points the same way. Every trial in this run is preceded by a warm-up message, and the
overall rate, 29.9%, is what the earlier run measured with no warm-up at all. Adding real traffic
immediately before a stream changed nothing.

**What this does not rule out**: if the effect existed but lasted longer than 20 s, every arm here
would be equally warm and the result would look flat for the wrong reason. Ruling that out needs gaps
of minutes, which costs a run long enough to hit the problem described in the next section.

## Run 1: what a broken measuring apparatus looks like

300 trials, rounds of 1, in an earlier window. Reported because it was nearly read as a result.

The headline said the failure rate was 59.3%, the wallet-visible rate 29.7%, and, worst of all, that
the conditional rates rose steadily: 59% → 70% → 80% → 90%. Read at face value that is the project
failing, with failures so correlated that retrying cannot help.

It was the local client dying. It warned `Not enough bandwidth` six minutes in, entered a congestion
loop about 40 minutes in (754 warnings of `sending_delay_controller: Trying to increase delay
multiplier higher than allowed`), and by the end was refusing every open instantly: the last 50 trials
took **zero** seconds between them. Of 178 first-attempt failures, 77 were opens the SDK refused
outright, not probes going unanswered.

Restricted to the first 200 trials, where no open was refused, the same run reads: 45.0% raw, 4.0%
wallet-visible, an 11.2x reduction, and **flat** conditional rates of 45.0%, 45.6%, 41.5%, 47.1%,
against 4.10% predicted by independence and 4.00% observed.

**Most of that was the machine, not the client.** Checking `pmset -g log` against all four runs
afterwards: this run and the warm-traffic one both had the host entering sleep every two minutes from
partway through, while the interleaved run above saw none at all and is the one that finished healthy
after nearly two hours and 670 streams. A client whose host suspends loses its gateway, and everything
after that is the laptop rather than the transport or the SDK.

One symptom survives that explanation. `Not enough bandwidth` was logged six minutes in, well before
the first suspension, so the bandwidth ceiling is real and separate. What it does over a long run is
untested.

Two things follow, and both are now in the code:

- **A refused open is not a failure of the transport**, and counting the two together turns a broken
  client into an apparently broken network with apparently correlated failures, because a broken
  client fails every round of every trial. They are reported apart, any refusal marks a run invalid,
  and a client that stops opening ends the run instead of finishing it.
- **A measurement host must be kept awake**, under `caffeinate` or equivalent, and its sleep log
  checked against the run afterwards. A suspension corrupts more than the failure rate: in the
  warm-traffic run above it would corrupt the independent variable itself, turning a 20-second gap
  into a gap of minutes.

## Run 2: why interleaving is not optional

200 trials, rounds of 3 only, run about an hour after run 1. It reported a 0.00% failure rate and a
p99 of 1,860 ms, which would have looked like a triumph for opening streams together.

It shows nothing of the sort, for two reasons.

**A per-stream rate cannot be measured with rounds larger than one.** A round ends as soon as one
stream answers, roughly 1.3 s in, and the others are cancelled about 9 s before their deadline would
have expired. The streams that were going to fail are dropped before they can, so the number is pinned
near zero by construction rather than by the network. The tool now says so instead of printing it.

**And with nothing to compare against in the same window, the result is unattributable.** Run 1 and
run 2 are an hour apart on a transport whose rate moves by an order of magnitude, so the improvement
could as easily be the weather. This is the same mistake that invalidated an earlier reply-block sweep
in the evaluation that preceded this project, arrived at from a different direction.

## Limitations

- One machine, one pair of gateways, one afternoon. The per-stream rate of 34.67% is one sample of a
  distribution that has been seen anywhere between 2% and 51%.
- 150 trials per arm. A round-of-three failure is rare enough at this rate that its count carries a
  wide interval: one observed event is compatible with a fair range of true rates.
- Only rounds of 1 and 3 were compared, and only at a 10 s deadline with the default reply-block
  budget. Whether 2 is enough, and how the deadline trades against round size, is unmeasured.
- The gaps tested for warm traffic reach 20 s. Anything slower-acting than that is invisible here.
- No run lasted more than about two hours, and two of the four were cut short by the host suspending.
  Nothing here says what a client does over a day, and the bandwidth ceiling seen at six minutes makes
  that a real question rather than a formality.
- Establishment was measured; throughput once a connection is carrying traffic was not.
