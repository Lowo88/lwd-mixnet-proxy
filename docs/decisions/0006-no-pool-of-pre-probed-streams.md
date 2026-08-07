# 0006. No pool of pre-probed streams

## Context

Probing costs a round trip before the wallet's first byte moves, and that round trip is seconds on
this transport rather than milliseconds. The obvious way to take it off the critical path is a pool:
keep streams that have already answered a probe ready, and hand one over the moment a wallet connects.

This was left open deliberately until there were numbers, because the size of the problem decides
whether the answer is worth its complexity. There now are numbers
([measurements](../measurements/2026-08-06-probe-and-retry.md)): with rounds of three, establishing a
connection measured p50 1,240 ms and p99 6,340 ms.

## Decision

**No pool. A stream is opened, probed, and used, in that order, per connection.**

The measured cost does not justify it. A wallet pays about 1.2 s once per connection, on a transport
where a single request costs seconds regardless, and this proxy is documented as being for the small
latency-insensitive calls rather than for bulk sync. Removing 1.2 s from the setup of a conversation
whose every round trip costs about as much is not the bottleneck.

The deeper reason is that a pool would not deliver what it appears to. **A stream that answered a
probe a minute ago is not a stream that works now.** The probe establishes that a round trip
completed at that moment; nothing about this transport makes that a durable property, and it carries
no close, so a pooled stream can die silently while it waits and look exactly like a live one. A pool
therefore trades a known 1.2 s for a stream of unknown freshness, and to get the guarantee back it
would have to re-probe on handout, which is the cost it was meant to avoid.

Holding pooled streams is not free either: each one occupies a registration on the listening half,
consumes reply blocks to stay addressable, and is one more thing to reap when its dialler goes away.

## Consequences

- Connection setup keeps a predictable shape: one round trip, paid once, visible in the numbers above.
- The 1.2 s is a floor for the current design. If a wallet ever needs a connection established faster
  than the transport's own round trip, that is a different problem than pooling solves.
- If the establishment cost ever becomes the complaint, the honest fix is fewer round trips rather
  than earlier ones: the probe could be dropped entirely
  ([0003](0003-probe-every-stream-before-the-wallet-uses-it.md) keeps that switch) if the transport
  stops losing first payloads.
- This decision rests on one afternoon's latency figures. A session where round trips are much slower
  would be worth re-reading it against, and the measurement to do so already exists.
