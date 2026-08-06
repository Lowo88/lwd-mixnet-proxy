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
number to record alongside the result, rather than as a condition of passing, would let that through.

## Decision

**Take both headline numbers from the same attempt, and pass or fail on four conditions rather than
one threshold.**

One trial is one dial with retry enabled, and it records whether the **first** stream answered, which
is what a wallet with no probe would have seen, and whether **any** stream answered, which is what a
wallet behind this proxy sees. They are the same sample: same second, same gateway, same conditions.
Interleaving configurations removes the drift between blocks; this removes it entirely, because there
is nothing to interleave.

The benchmark is a thin binary over the same `dial` the client half calls. Measuring the production
code path rather than a parallel implementation is the point: a rig that reimplements the retry can
be right about a mechanism that does not ship.

A run counts, and S0 passes, on all of:

1. **The run is powered.** At least 300 trials, with a raw failure rate of at least 10%. Below that
   there are too few raw failures for the retry to be doing anything measurable, and the result must
   be reported as unmeasured rather than as passing.
2. **Failures are independent enough to retry.** The failure rate of round two, conditional on round
   one having failed, is not substantially above the rate of round one. This is the load-bearing
   condition: it is a property of the transport rather than of the afternoon, so from it the attempts
   needed for any raw rate can be computed, including rates that day did not produce.
3. **The reduction is a factor, not a threshold.** At least tenfold between the raw and visible rates,
   which asks the same thing of a good day and a bad one.
4. **Establishment stays inside a budget a wallet tolerates.** p50 at or under 5 s and p99 at or under
   30 s, measured end to end with retries included. This condition is equal to the others, not
   subordinate: failing it fails S0 even if 2 and 3 pass.

The benchmark reports the per-round conditional rates and the spread of failures inside a round
directly, because those two answer condition 2 from both sides: rounds are separated in time by a
deadline, while streams inside one round are simultaneous, so comparing them separates correlation
that decays with time from correlation that belongs to a moment.

## Consequences

- The comparison holds while the transport is degrading, which is the condition it most needs to
  survive, and a quiet afternoon is reported as uninformative rather than as a pass.
- The retried rate is conditional rather than independent: later attempts happen only because earlier
  ones failed. That is a feature. If failures are correlated, retry does not help, and condition 2
  says so instead of assuming otherwise.
- Sequential retry and hedged opening are the same code under different configuration, so both can be
  measured in one session and compared without the drift that would invalidate separate runs.
- The latency budget is a design input, not an outcome: the deadline and the number of attempts are
  chosen to fit it, rather than being chosen first and measured afterwards.
- What the probe costs is read off the same run: the elapsed time of an answering round is the round
  trip the probe adds to establishing a connection.
- A measured stream is never used, and the listening half connects to its upstream only when a request
  arrives, so a benchmark run never reaches the node behind it.
