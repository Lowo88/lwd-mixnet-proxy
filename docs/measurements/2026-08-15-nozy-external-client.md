# What an external dialling half saw against the public testnet

Date: 2026-08-15, with a longevity check on 2026-08-16. Not a same-machine loopback
run. This is one independent operator's client half pointed at the public testnet
serving address, with raw counters and a small interleaved bench in the same window.

Author: LaDale / Nozy wallet (forum user Lowo88). Written for
[the forum thread](https://forum.zcashcommunity.com/t/lwd-mixnet-proxy-light-wallet-grpc-over-the-nym-mixnet-and-what-three-days-of-measuring-it-found/57000/7)
after Joaco asked for a measurement PR under `docs/measurements/`. The serving-half
side of the same afternoon is already correlated in
[The gateway allowance empties at 00:00 UTC](2026-08-15-daily-bandwidth-cliff.md).

## Method

`docker compose` client-only against Joaco's published public testnet `SERVER_ADDRESS`.
Image built from this repo at the time of the run; Nym SDK **1.21.5-rc.3**. Entry
gateway chosen by the client: `DuMkz6bVpKnZnbWf5DYtKHFhaLUUzB4vNSqdoiVR4j8X`
(node_id 2762, `ws://147.79.68.95:9000/`). Host: Windows + Docker Desktop. Compose
`restart: unless-stopped`.

UTC timestamps for the load window:

| event | time |
|---|---|
| client up (`compose up -d client`) | 2026-08-15T16:07:56Z |
| health reported serving | 2026-08-15T16:11:22Z |
| `/metrics` snapshot after load | 2026-08-15T16:18:05Z |

Load behind `127.0.0.1:9068` was **TCP connect/close only** (no LWD gRPC bytes): 30
connections. Mixnet probe establish/discard counters still moved. Separately,
`lwd-mixnet-bench --trials 20` with interleaved rounds of 1 and 3 ran in a one-off
container against the same serving address in the same afternoon.

Raw files:

- [`raw/2026-08-15-nozy-client-metrics.txt`](raw/2026-08-15-nozy-client-metrics.txt) — rates, timestamps, `/metrics` block
- [`raw/2026-08-15-nozy-bench.txt`](raw/2026-08-15-nozy-bench.txt) — bench trial table and summary
- [`raw/2026-08-16-nozy-overnight.txt`](raw/2026-08-16-nozy-overnight.txt) — ~24h check

The dialled Nym address in the raw files is replaced with the label
`public-testnet-server`, same redaction idea as the 2026-08-06 bench files.

## What the client counters said

At 2026-08-15T16:18:05Z:

| counter | value |
|---|---|
| `connections_total` | 30 |
| `first_round_failures_total` | 5 |
| `connections_unestablished_total` | 2 |
| `rounds_total` | 35 |
| `streams_opened_total` | 95 |
| `streams_discarded_total{reason="unanswered"}` | 17 |
| `establishment_seconds_count` | 28 |

Rates against `connections_total`:

- first-round failure: **16.67%** (5/30)
- wallet-visible failure: **6.67%** (2/30)

The unanswered discard count matches a simple ladder of round sizes used that day:
5 × 3 + 2 × 1 = 17. Bytes to/from the mixnet stayed 0, which is expected for TCP
connect/close with no RPC payload.

## What the interleaved bench said

20 trials, 10 per arm, rounds of 1 and 3 interleaved, 10 s probe deadline. No opens
refused.

| | rounds of 1 | rounds of 3 |
|---|---|---|
| first-round failure rate | **60.00%** (6/10) | **10.00%** (1/10) |
| wallet-visible failure rate | **0.00%** (0/10) | **0.00%** (0/10) |
| establishment p50 | 11,171 ms | 1,392 ms |

Same shape as the 2026-08-06 harness report: retry clears what the wallet sees, and
rounds of three cut the establishment tail. Absolute rates are one short sample in
one window; they are not a claim that the transport sits at 60%.

## Longevity overnight

At 2026-08-16T16:25Z the same container was still healthy and serving, about 24h17m
after start. Docker `RestartCount` was 0. Connection counters were unchanged from the
16:18Z snapshot (idle overnight; no further dials).

At **2026-08-16T00:00:01Z** the client logged **14** `Not enough bandwidth` warnings
and then claimed testnet bandwidth successfully in the same second. That lines up
with the serving-half midnight cliff in
[2026-08-15-daily-bandwidth-cliff.md](2026-08-15-daily-bandwidth-cliff.md): two
independent clients, two gateways, same UTC second.

## Correlated on the serving half

Joaco already noted on the forum and in the cliff report that the deployment's
`duplicate fragment received` warnings all fall between **16:11:29Z and 16:16:43Z**
on 15 aug — inside this dialling window — and that the midnight reclaim on 16 aug
matches this client's log. Credit for the serving-half reading is theirs; this PR
is the dialling-half numbers they asked for.

## Limitations

- TCP connect/close only on `:9068`, so this is not an end-to-end LWD gRPC call over
  the mixnet. Probe metrics moved; application bytes did not.
- Small samples: 30 local connections, 20 bench trials.
- Overnight was idle cover traffic. No dial was in flight inside the 00:00 UTC reclaim
  window, so the cost to a live stream is still unobserved from this side.
- One machine, one entry gateway, one afternoon's weather.
- The client only reported healthy at 16:11:22Z after coming up at 16:07:56Z; early
  traffic may still have been settling.