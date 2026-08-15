# The gateway allowance empties at 00:00 UTC

Date: 2026-08-15. Not a bench run. This is the public testnet serving half in ordinary operation, read
from its own log and `/metrics`.

## Method

One `lwd-mixnet-server` container in front of a testnet `lightwalletd-rs`, started 2026-08-13 14:04:29
UTC and still on its first process 51 hours later. Nothing was staged and nothing was driven: the
numbers below are whatever the deployment did while it sat there.

Both events happened with no wallet connected. Over the whole 51 hours the half opened one upstream
connection, during a smoke test on 13 aug, and `lwd_mixnet_server_connections_in_flight` was 0 at both
midnights. What the gateway refused to send was therefore cover traffic, not anyone's request.

The 106 log lines the two events produced are in
[`raw/2026-08-15-bandwidth-cliff.log`](raw/2026-08-15-bandwidth-cliff.log).

## What happens

Twice in 51 hours the gateway reported the client's bandwidth allowance at zero. Both times at
00:00:00 UTC, within a millisecond of each other:

| | 14 aug | 15 aug |
|---|---|---|
| first `run out of bandwidth` | 00:00:00.078109Z | 00:00:00.079051Z |
| `managed to claim testnet bandwidth` | 00:00:00.298429Z | 00:00:00.357774Z |
| time at zero | **220 ms** | **279 ms** |
| sphinx packets the gateway refused | 11 | 9 |
| claim attempts | 12 | 10 |
| last line of the window | 00:00:00.442611Z | 00:00:00.591272Z |

The shape is identical on both nights:

```
00:00:00.078109  WARN  run out of bandwidth when attempting to send the message! we got 0.00 B
                       available, but needed at least 2.36 kiB to send the previous message
00:00:00.087865  WARN  Not enough bandwidth. Trying to get more bandwidth, this might take a while
00:00:00.112058 ERROR  Failed to send sphinx packet(s) to the gateway: gateway returned an error
                       response: insufficient bandwidth available to process the request.
                       required: 2413B, available: 0B
00:00:00.298429  INFO  managed to claim testnet bandwidth
```

Two details are worth keeping.

**The claims go out in parallel and only one is answered.** Every `Not enough bandwidth` starts its
own attempt, one succeeds, and the replies to the others arrive as `received illegal message of type
'Bandwidth' in an authenticated client`. That count is attempts minus one on both nights, 11 of 12 and
9 of 10, so the warning is the price of firing them together rather than a fault.

**This is free testnet bandwidth.** The client runs in what the SDK calls disabled credentials mode
and claims its allowance without a credential, which is also why recovery is immediate. A mainnet
client pays with ticketbooks (`Ticketbooks stored: 0` in this log, every hour), so nothing here says
what the same second looks like when the allowance has to be bought.

## Why a 250 ms outage is worth writing down

The refusal is returned to the sender, logged, and that is the end of it. Nothing above that layer is
told: no closed stream, no error on a write, no timeout that fires early. A stream whose first payload
sits in one of those refused packets ends up in exactly the state
[ADR 0003](../decisions/0003-probe-every-stream-before-the-wallet-uses-it.md) and
[ADR 0004](../decisions/0004-deadlines-are-the-only-close.md) are built around: `accept()` fires on the
far side and the `read` never returns.

That link is inference. Both events landed on an idle server, so no stream was observed dying in one,
and it stays inference until someone catches a dial inside the window.

It is also far too small to be the phenomenon. A quarter of a second a day is around three parts per
million; per-stream failure rates between 2% and 51% are not made of this. What it gives is one
confirmed way for the transport to turn a payload into silence, on a schedule you can point at.

## The rest of the 51 hours

Counters at 2026-08-15 17:34 UTC, over the whole process lifetime:

| | |
|---|---|
| streams that arrived from the mixnet | 150 |
| never delivered a handshake inside the 30 s deadline (`streams_rejected_total{reason="unanswered"}`) | 67, 45% |
| introduced themselves but carried no request (`streams_without_request_total`) | 82 |
| reached the upstream | 1 |

The sample is small and most of it is one machine's testing, so 45% is an observation and not a rate.
It is the same first-payload loss the bench measured from the dialling side, seen from the receiving
end for the first time.

The log is quiet otherwise: 22 `Not enough bandwidth` warnings in 51 hours, every one of them inside
the two seconds above, 13 duplicate reassembly fragments, and one failed topology refresh that kept
the previous topology and moved on.

## What this settles, and what it does not

The [2026-08-06 report](2026-08-06-probe-and-retry.md) ends by saying that nothing in it shows what a
client does over a day, and that `Not enough bandwidth` six minutes into a run makes that a real
question. Part of the answer: a gateway registration survived 51 hours without a restart, and both
times the allowance emptied, the client refilled it by itself in under 300 ms. Whatever ended those
two-hour runs, it was not this.

## Limitations

- One serving half, one gateway, two events. Whether the reset belongs to that gateway, to testnet
  accounting, or to the network is not established here.
- Testnet and free claims. The mainnet path through ticketbooks is unmeasured.
- Neither event had traffic on it, so the cost to a live stream is reasoned, not observed.
- 51 hours is not weeks, and the question was about weeks.
- The window edges are the first and last lines the client logged. How long the gateway had been at
  zero before the client tried to send is not visible from here.
