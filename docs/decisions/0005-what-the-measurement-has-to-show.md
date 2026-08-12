# 0005. What the measurement has to show, and how it is taken

## Context

Whether this project is worth running comes down to one comparison: how often a stream fails, against
how often a wallet notices. If the transport fails 30% of the time and a wallet behind this proxy
still fails 25% of the time, the design does not work and no amount of packaging changes that.

Three things make that comparison harder than it looks.

**The failure rate is not stationary.** It measured 2% one afternoon and 36.5% the next day at
identical settings, and nearly tripled between the first and second halves of a single run. An
earlier experiment compared configurations by running each as its own block and had to be discarded:
drift between blocks was larger than the effect being measured.

**A threshold on the visible rate mostly measures the afternoon.** Assuming independence, k attempts
turn a raw rate p into p^k, so reaching 1% takes 2 attempts at p=0.02 and 5 at p=0.365. A fixed "under
1%" bar is therefore nearly automatic on a good day and unreachable on a bad one with the same
configuration, and passing it proves whichever day it was run.

**Retry multiplies only if failures are independent**, and there is no reason to assume they are. A
degraded window or a bad gateway would correlate them, and then the arithmetic above does not hold at
all: no attempt count is enough, and the design fails no matter what a headline rate says.

There is a fourth thing, and it is the one that can end the project quietly. Because a failure is
silent, it is only discovered when the probe deadline expires, so **every retry is paid for in
seconds of establishment latency**. A configuration can clear any reliability bar and still make a
wallet wait 47 seconds before its first byte moves, which no wallet tolerates. Treating latency as a
number to record alongside the result, instead of as a condition of passing, would let that through.

## Decision

**Take both headline numbers from the same attempt, and accept or reject on a set of conditions, not
on one threshold.**

One trial is one dial with retry enabled, and it records whether the **first** stream answered, which
is what a wallet with no probe would have seen, and whether **any** stream answered, which is what a
wallet behind this proxy sees. For this pair there is nothing to interleave: they are the same sample,
same second, same gateway, same conditions. Comparing two configurations is a different problem, and
is dealt with below.

The benchmark is a thin binary over the same `dial` the client half calls. Measuring the production
code path is the point: a rig that reimplements the retry can be right about a mechanism that does
not ship.

A run counts, and the design is accepted, on all of:

0. **The run is clean.** No open was refused by the local client. A refused open is this machine
   failing, reported instantly, while the loss being filtered here is silent. Counted together, a
   degrading client reads as a degrading network whose failures have become correlated, because a
   broken client fails every round of every trial. Any refusal invalidates the run, and a client that
   stops opening altogether ends it rather than finishing it.
1. **The run is powered.** At least 300 trials, with a raw failure rate of at least 10%. Below that
   there are too few raw failures for the retry to be doing anything measurable, and the result must
   be reported as unmeasured, not as passing.
2. **Failures are independent enough to retry.** The failure rate of round two, conditional on round
   one having failed, is not substantially above the rate of round one. This is the load-bearing
   condition: it is a property of the transport rather than of the afternoon, so from it the attempts
   needed for any raw rate can be computed, including rates that day did not produce.
3. **The reduction is a factor, not a threshold.** At least tenfold between the raw and visible rates,
   which asks the same thing of a good day and a bad one.
4. **Establishment stays inside a budget a wallet tolerates.** p50 at or under 5 s and p99 at or under
   30 s, measured end to end with retries included. This condition is equal to the others, not
   subordinate: failing it rejects the design even if 2 and 3 pass.

The benchmark reports the per-round conditional rates and the spread of failures inside a round
directly, because those two answer condition 2 from both sides: rounds are separated in time by a
deadline, while streams inside one round are simultaneous, so comparing them separates correlation
that decays with time from correlation that belongs to a moment.

**Configurations are interleaved trial by trial, and the same-attempt argument does not extend to
them.** Crude and visible rates come from one attempt and need no interleaving. Two round sizes
cannot: they are separate dials, and run as separate sessions they are an hour apart on a transport
that moves by an order of magnitude, so the difference between them is indistinguishable from the
weather. Rotating them one per trial is therefore the only way the comparison means anything.

**A per-stream rate is not measurable with rounds larger than one**, and the tool says so instead of
printing a number. A round ends as soon as one stream answers, and the rest are cancelled seconds
before their deadline would have expired, so the streams that were going to fail are dropped before
they can. Only the round is measurable at that size, which is why the round is the unit compared
across configurations.

## Consequences

- The comparison holds while the transport is degrading, which is the condition it most needs to
  survive, and a quiet afternoon is reported as uninformative, not as a pass.
- The retried rate is conditional rather than independent: later attempts happen only because earlier
  ones failed. That is a feature. If failures are correlated, retry does not help, and condition 2
  says so instead of assuming otherwise.
- Sequential retry and hedged opening are the same code under different configuration, so both can be
  measured in one session and compared without the drift that would invalidate separate runs.
- The latency budget is a design input, not an outcome: the deadline and the number of attempts are
  chosen to fit it.
- What the probe costs is read off the same run: the elapsed time of an answering round is the round
  trip the probe adds to establishing a connection.
- A measured stream is never used, and the listening half connects to its upstream only when a request
  arrives, so a benchmark run never reaches the node behind it.

## Outcome

The run is in [`../measurements/2026-08-06-probe-and-retry.md`](../measurements/2026-08-06-probe-and-retry.md).
The design is accepted with rounds of three: failures between rounds proved independent, a 34.67% per-stream
failure rate reduced to nothing observable in 300 trials, and establishment held a 6.3 s p99. Retrying
in series cleared every condition except the latency budget, at a 31.3 s p99.

Conditions 0 and the interleaving requirement above were added because of that run rather than before
it: the first attempt at measuring reported the project failing, and the second reported it succeeding,
and neither was measuring what its headline claimed.
