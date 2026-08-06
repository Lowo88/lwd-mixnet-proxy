# 0001. Ship this as its own repository, as two binaries

## Context

This project began as the follow-on to an evaluation carried out inside
[`lightwalletd-rs`](https://github.com/jpgonzalezra/lightwalletd-rs), which concluded that a mixnet
transport does not belong in that crate: the SDK is larger than the crate itself, cannot be trimmed,
and would enter every build, CI job and downstream lockfile for the benefit of the operators who want
it. The transport was to be a separate process instead.

That leaves where the separate process should live. Keeping it under `contrib/` in the same
repository is the smaller step, and would have been wrong for a reason that has nothing to do with
build size: **the serving half takes its upstream from configuration and knows nothing about what
answers there.** It is a TCP endpoint that speaks the light-client protocol, and any implementation
of that protocol qualifies. Placing it inside one implementation's repository would present shared
infrastructure as that implementation's accessory.

The two halves also have opposite deployment stories. One runs next to a wallet, on a laptop or a
phone's desktop companion, and wants no identity at all. The other runs next to a server, wants a
stable identity on disk, and is supervised. They share a protocol and almost no operational
concerns.

## Decision

**A separate repository, with two binaries and a small library between them.**

- `lwd-mixnet-client` runs next to the wallet: a local TCP port in, mixnet out.
- `lwd-mixnet-server` runs next to the server: mixnet in, a configurable TCP upstream out.
- The library holds what both need and what deserves tests: the handshake, the dialling loop, and
  the byte pump with its deadlines.

Neither half parses gRPC, and neither has a reason to. A mixnet stream implements
`AsyncRead + AsyncWrite`, so gRPC travels through untouched.

## Consequences

- The serving half is usable in front of any light-client server, and nothing here needs to know
  which one an operator runs.
- Versioning, CI and releases follow this project's own cycle rather than being tied to a server's.
- The measurement rig that produced the evidence stays frozen where it is, in the other repository,
  because the documents that cite it are there. This project starts from its findings, not its code.
- The cost is a second repository to maintain, and a protocol between the halves that has to stay
  compatible with itself across releases. The handshake carries a version byte for that reason.
