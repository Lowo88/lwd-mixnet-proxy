# 0011. Capacity is the only abuse control

## Context

Every abuse control the serving half might want starts from knowing who is on the other end. Rate
limit per address, ban a repeat offender, price a client by what it has already asked for — all of
them need an identity, and the transport is built to destroy exactly that. A stream arrives from the
mixnet with no source address, no key, nothing that survives from one stream to the next. A flooder
and a crowd of wallets look the same on purpose.

What the half had until now was no control at all: accepted streams went into an unbounded task set.
Each one holds a task through the handshake deadline, and one that introduces itself holds an 8 KiB
buffer for up to the first-request deadline before it has to say anything. Nothing bounded how many
connections that could turn into upstream, either. A flood costs the host tasks and descriptors and
costs the node connections, and neither ends until the deadlines do.

## Decision

**One semaphore at accept, `--max-streams`, 256 by default.** A stream that cannot take a permit is
dropped before its handshake is read, counted as
`lwd_mixnet_server_streams_rejected_total{reason="over_capacity"}`, and logged.

**Drop, do not queue.** A queued stream would sit there ageing toward the probe deadline its dialler
is already counting down, and arrive dead.

**The permit is held by the serving task**, so it comes back when the stream is let go — by the
handshake deadline, the first-request deadline, or the idle deadline. The cap therefore bounds
upstream connections as well, since a stream must hold a permit to reach the upstream at all.

**No per-peer accounting of any kind**, because there is no peer to account against.

## Consequences

- A flood is bounded by the number the operator chose, and it is visible: the rejected counter
  separates it from handshake failures, so shedding does not hide inside the ordinary noise.
- The flooder pays too. Every dropped stream cost it the reply blocks it attached, which is the one
  price this transport does charge.
- A legitimate dialler that arrives at a full house sees an unanswered probe — the failure it
  already retries from, and the one this project is built around (ADR 0004). It is not told the
  house is full, because there is no way to tell it.
- 256 is a guess sized for a small host, not a measurement. It is a flag for that reason; someone
  who measures what a held stream really costs should revise the number, not the mechanism.
- The cap is per process. Running two serving halves behind one address is not a thing this
  supports, so there is nothing to coordinate.
- If the SDK ever exposes something durable about a sender, a fairer scheme becomes possible and
  this record should be superseded rather than quietly amended.
