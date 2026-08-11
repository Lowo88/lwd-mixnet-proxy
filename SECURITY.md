# Security policy

## Reporting

Use GitHub's private vulnerability reporting on this repository: "Security" tab → "Report a
vulnerability". Please don't open a public issue for a suspected vulnerability.

## Scope

**In scope**: the dialling half and the serving half, the handshake and splice protocol between
them, the container image, and the compose file.

**Out of scope**: the mixnet's own delivery and anonymity properties — report those to the Nym
project — and the upstream light-client server behind the proxy.

## Threat-model pointers

- The state directory is private key material: see README, ["The state directory is private key
  material"](README.md#the-state-directory-is-private-key-material).
- The wallet-facing port and the metrics endpoint are unauthenticated by design and belong on
  loopback or a private network: see README's ["Watching it
  run"](README.md#watching-it-run) and [ADR
  0007](docs/decisions/0007-report-a-pair-of-rates-over-an-endpoint-that-is-off-by-default.md).
- Syncing and submitting through the same server lets an operator correlate the two by timing:
  see README, ["Use it for the small calls, not for
  syncing"](README.md#use-it-for-the-small-calls-not-for-syncing).

## Dependency posture

The SDK is pinned exactly and `Cargo.lock` is committed ([ADR
0002](docs/decisions/0002-pin-the-sdk-and-ship-a-container.md)). Known advisories in the pinned
tree are recorded with justifications in [`.cargo/audit.toml`](.cargo/audit.toml) and re-evaluated
at every SDK bump. A scheduled CI job runs `cargo audit` and fails only on an advisory that isn't
already in that file. The reasoning is [ADR
0009](docs/decisions/0009-watch-advisories-against-a-pinned-tree.md).
