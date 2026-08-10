# Architecture decision records

Short records of the decisions that shape `lwd-mixnet-proxy`. Each ADR captures one decision in a
fixed format, **Context**, **Decision**, **Consequences**, so the reasoning behind a choice stays
discoverable after the fact. The living overview is [`../ARCHITECTURE.md`](../ARCHITECTURE.md); these
records explain why it looks the way it does.

| ADR | Decision |
|---|---|
| [0001](0001-its-own-repository.md) | Ship this as its own repository, as two binaries |
| [0002](0002-pin-the-sdk-and-ship-a-container.md) | Pin the SDK exactly, commit the lockfile, ship a container |
| [0003](0003-probe-every-stream-before-the-wallet-uses-it.md) | Probe every stream before the wallet is allowed near it |
| [0004](0004-deadlines-are-the-only-close.md) | Deadlines are the only close |
| [0005](0005-what-the-measurement-has-to-show.md) | What the measurement has to show, and how it is taken |
| [0006](0006-no-pool-of-pre-probed-streams.md) | No pool of pre-probed streams |
| [0007](0007-report-a-pair-of-rates-over-an-endpoint-that-is-off-by-default.md) | Report a pair of rates, over an endpoint that is off by default |
