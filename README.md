# lwd-mixnet-proxy

Carry a Zcash light wallet's gRPC connection over the [Nym](https://nym.com) mixnet, without changing
the wallet or the server.

Two processes, each a byte pipe:

```
  wallet  --TCP-->  [lwd-mixnet-client]  --mixnet-->  [lwd-mixnet-server]  --TCP-->  lightwalletd
```

Neither half understands gRPC. A mixnet stream implements `AsyncRead + AsyncWrite`, so the connection
travels through unmodified: the wallet is pointed at a local port, and the server sees an ordinary
client. The serving half takes its upstream from configuration, so it works in front of any
implementation of the light-client protocol.

> **Status: early.** The core mechanism is implemented, unit-tested, and measured against the live
> mixnet: under a 34.67% per-stream failure rate it reduced what a wallet sees to 0 of 300
> connections, with a 6.3 s p99 to establish. That is one afternoon on one network path, and the
> operational surface around it (metrics, health checks, packaging) is not built yet.

## Why

A light wallet reveals more to a server than the protocol suggests, and the leak is roughly inverse to
the bandwidth: bulk block download is the heaviest call and the least revealing, because the client
fetches everything and trial-decrypts locally, while the cheapest calls are the ones worth protecting.
Submitting a transaction links it to a network identity. Transparent-address queries hand over the
addresses directly.

A mixnet routes each packet separately through several relays that delay and reorder traffic, so it
resists the timing correlation a low-latency overlay does not. That makes it a good fit for exactly
the calls that leak the most and cost the fewest bytes.

## What this is really for

The transport has a defect that decides the whole design: **a stream can open, be accepted by the far
side, and silently lose its first payload.** Neither end errors and neither times out; both hang. The
rate measured between 2% and 51% over three days and is not stationary.

gRPC libraries recover from errors. They do not recover from silence. So this proxy exists to convert
that hang into an invisible retry, and in the worst case into a fast error:

- Before the wallet sends a byte, the dialling half **probes** each stream and discards any that does
  not answer within a deadline. The probe reproduces the measured failure exactly, so it filters it by
  construction.
- Streams are opened **three at a time**, keeping the first to answer. Since a silent failure is only
  discovered when the deadline expires, retrying one at a time pays that deadline per failure and
  drags the tail out: measured side by side, that difference was a p99 of 31.3 s against 6.3 s.
- Once bytes are moving, a **watchdog** closes a connection whose far side stopped answering, so the
  wallet's own gRPC library reconnects. Recovering an in-flight request is not promised; **never
  hanging forever** is.

## Use it for the small calls, not for syncing

At the measured throughput a full historical sync would move tens of gigabytes over a transport whose
median round trip is seconds, which is on the order of days. This is not a drop-in replacement for an
ordinary connection.

**Worth carrying:** transaction submission, transparent-address queries, single transaction lookups.
High leak, few bytes, latency-insensitive.

**Not worth carrying:** bulk block download, unless the wallet's birthday is recent.

**Do not sync and submit through the same server.** An operator that sees an address synchronising and
moments later receives an anonymous transaction can correlate the two by timing, and with few
concurrent users the anonymity set is negligible. Using a different instance for each costs nothing.

## Running

Both halves need to reach the mixnet. The serving half prints the address the dialling half needs.

```
# next to the server
lwd-mixnet-server --upstream 127.0.0.1:9067 --state-dir /var/lib/lwd-mixnet
# NYM_ADDRESS=<identity>.<encryption>@<gateway>

# next to the wallet
lwd-mixnet-client --server <that address> --bind 127.0.0.1:9068
```

Then point the wallet at `127.0.0.1:9068`.

Every flag has an environment variable, listed in `--help`. The ones that matter:

| flag | what it trades |
|---|---|
| `--probe-timeout-secs` | Healthy round trips have a long tail, so a short deadline discards working streams; a long one makes each failure expensive. There is no value that is good at both. |
| `--probe-attempts` | Total streams one connection may open before giving up. |
| `--probe-concurrency` | Streams opened at once, three by default. One retries in series and pays a deadline per failure; three keeps the tail short at the cost of three streams and three reply-block budgets per connection. |
| `--reply-surbs` | Raising it lowers the failure rate without reaching zero, and costs latency. A knob, not a fix. |
| `--stall-timeout-secs` | How long the wallet waits on an answer that is not coming before the connection is closed. |

### The state directory is private key material

The serving half's Nym address is derived from the keys in `--state-dir`. Losing it changes the
address, so clients can no longer find it. Copying it allows impersonation. It is gitignored here and
belongs in a volume with restricted permissions.

### Cost of being connected

A connected mixnet client generates continuous cover traffic, on the order of 2 Mbps sustained, for as
long as it is running. That is what the traffic analysis resistance is made of, but it is a real bill
on a metered connection.

## Building

```
make build     # cargo build
make test      # unit tests, no network
make lint      # clippy, warnings denied
make fmt       # rustfmt check
make verify    # all of the above
make image     # release binaries in a container
```

The SDK is pinned exactly and `Cargo.lock` is committed. It resolves to roughly 750 packages and
cannot be trimmed; upgrading it is a deliberate change, and whoever does it should re-run the
measurement rather than trust the numbers recorded here.

## Measuring

`lwd-mixnet-bench` drives the same dialling code the client half uses, against a running serving half,
and reports the raw failure rate and the wallet-visible one from the same attempts, along with what
establishing costs and whether failures are independent enough for retrying to help at all.

```
lwd-mixnet-bench --server <address> --trials 300 --attempts 6 --concurrency 1,3
```

Round sizes given as a list are rotated one per trial. That is not a convenience: the transport's
failure rate moves by an order of magnitude between one hour and the next, so two configurations
measured in sequence cannot be told apart from the weather.

What a run has to show, and why a single threshold would not have been enough, is in
[ADR 0005](docs/decisions/0005-what-the-measurement-has-to-show.md). The results so far, including two
runs that had to be thrown out and what they teach, are in
[`docs/measurements/`](docs/measurements/2026-08-06-probe-and-retry.md).

## Documentation

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — what the pieces are and how bytes move.
- [`docs/decisions/`](docs/decisions/README.md) — why it looks like this.
- [`docs/measurements/`](docs/measurements/2026-08-06-probe-and-retry.md) — what was measured, and what it cost to measure it properly. Raw output from every run is kept alongside it.

## Acknowledgments

The evaluation that produced this design was carried out in
[`lightwalletd-rs`](https://github.com/jpgonzalezra/lightwalletd-rs), where the measurements and the
decision to keep the transport out of that crate are recorded. Thanks to the Zcash community, and to
Nym for the SDK this is built on.

## License

MIT. See [LICENSE](LICENSE).
