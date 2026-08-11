# 0010. Exit when the local client degrades

## Context

A mixnet client can stop working without saying so. The measurement log for
[2026-08-06](../measurements/2026-08-06-probe-and-retry.md) has a run where the client warned about
bandwidth, fell into a congestion loop, and from then on refused every `open_stream` instantly — the
last fifty trials took zero seconds between them. Nothing recovered it but restarting the process.

The dialling half builds its client once at startup and never looks at it again. In the state above
it keeps accepting wallet connections and closing every one of them while `/health` still answers
`serving`: an outage that reads as a healthy process, which is the one shape no supervisor reacts
to. The measurement binary already stops itself on that signature; the daemon that runs for weeks
does not.

## Decision

**Count consecutive connections in which the SDK refused every open, and exit non-zero at five.**
The dialling half logs at error level, drains what is in flight, and returns an error from `main`,
so `restart: unless-stopped` and any other supervisor do the one thing known to help.

**Only refusals across a whole dial count.** A dial whose probes all went unanswered opened
streams, which means the client is talking and the transport is losing — the ordinary bad day this
proxy exists for. Any connection that opens at least one stream resets the streak to zero.

**No attempt to rebuild the client in place.** What a half-dead client does with a reconnect is not
specified anywhere we can rely on; exit-and-restart is the recovery that was actually observed to
work.

**The serving half is left alone.** Its mixnet listener returning `None` already drains and ends the
process, and a silently dead server-side client behind a live listener has not been seen.

## Consequences

- A degraded client costs a restart instead of an invisible outage, and `/health` goes with the
  process rather than lying about it.
- Detection needs traffic. With no wallet connecting there is no streak and no exit, which is the
  right trade: an idle proxy is not a broken one.
- Under traffic it is near-immediate, because a client in this state refuses in milliseconds.
- Anything running this half must restart it on failure. The compose file does; a bare
  `docker run` does not.
- Five refusals in a row from a *healthy* client would exit for nothing. No such run has been
  observed, and the cost of being wrong is one reconnect.
