# 0007. Report a pair of rates, over an endpoint that is off by default

## Context

The transport's failure rate moved between 2% and 51% across three days of measurement and is not
stationary ([measurements](../measurements/2026-08-06-probe-and-retry.md)). An operator therefore has
no baseline: any single number this proxy reports is as much a statement about the afternoon as about
the deployment, and there is no threshold that means the same thing twice.

That is not a hypothetical. Three separate times while measuring, a plausible, well-formatted headline
number turned out to answer a different question than the one asked, and two of those nearly ended the
project or declared it finished. The apparatus was at fault each time, not the transport.

Something has to be decided about what is exported, how it is served, and whether it is served at all.

## Decision

**Export a pair of rates, not a headline, and serve them over an endpoint that is off unless asked
for.**

**The pair.** The dialling half counts, from the same dial: how often the streams it opened together
first all went unanswered (`first_round_failures_total`), and how often it ended up with nothing to
hand the wallet (`connections_unestablished_total`). Both are divided by `connections_total`. The
first is the transport and drifts on its own; the second is what this project exists to keep near
zero. A rising first with a flat second is a bad afternoon working as designed; both rising together
is this proxy failing. Neither reading is available from either number alone.

Counting both from the same attempt is what makes them comparable. Measured in separate runs they
would be two samples of two different moments, on a transport that moves by an order of magnitude
between one hour and the next.

**`hyper` and `prometheus`, not a framework.** Both are already in the dependency tree by way of the
SDK, so declaring them directly adds **no package to the lockfile**, which stays at 757. The
exposition format's escaping and its cumulative histogram buckets come from a library that is already
being compiled rather than from code of ours that would need its own tests. `axum` would have been
more pleasant to write and is the only candidate that would have grown the tree.

**Off by default, on both halves.** `--metrics-bind` has no default value. The dialling half runs on
the same machine as the wallet, where a port that appears without being asked for is a surprise; the
listening half is someone's server, where the operator is the one who decides what listens.

**Three startup states.** `starting`, `registered`, `serving`, served on `/health` with a 200 only for
the last. Registration against a gateway takes seconds and was observed to fail outright on 2 of 15
attempts, and "still coming up" and "up and broken" call for different reactions from whoever is
watching. The endpoint is therefore bound *before* the mixnet client connects, so there is something
to ask during the part of startup that is slow and unreliable.

## Consequences

- An operator can tell a degraded network from a broken deployment. It costs two counters instead of
  one.
- Nothing is exported that identifies a client. The counters are counts of streams and connections;
  no peer, no address, nothing about what a wallet asked. The endpoint is still unauthenticated, so it
  belongs on loopback or a private network: it reveals that this machine runs the proxy and how busy
  it is.
- A deployment that does not set `--metrics-bind` is flying blind, which is a real cost of making it
  opt-in. The README says so where the flag is documented.
- `registered` is reported but, as built, is very nearly unobservable: nothing slow sits between
  registering and serving, so a scrape will practically always catch `starting` or `serving`. It is
  kept because it names the boundary rather than because it is often seen.
- The pair is only as good as the definition of a round. If the dialling half ever changes how it
  groups attempts, `first_round_failures_total` changes meaning, and any recorded history of it stops
  being comparable.
- Two dependencies that were transitive are now direct, so an SDK upgrade that drops either becomes
  our problem to resolve rather than a silent removal.
