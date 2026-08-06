# 0002. Pin the SDK exactly, commit the lockfile, ship a container

## Context

`nym-sdk` resolves to roughly 750 packages, more than most projects that depend on it, and none of
that tree can be trimmed: SOCKS5, an IP router, a chain client and the credential machinery are all
mandatory. Over a three-week evaluation window the project deprecated its standalone TCP proxy,
deprecated its own Zcash demo, and paused then resumed publishing to crates.io. The evaluation ran
against a release candidate, `1.21.5-rc.3`, because it was the version where the measurements were
taken.

A dependency that large and that mobile is the main maintenance risk this project carries, larger
than anything in its own code.

Running the halves is a separate question from building them. The serving half holds a private key
directory that determines its address, and every connected client generates continuous cover traffic
of roughly 2 Mbps for as long as it is up.

## Decision

**Pin `nym-sdk` with `=` and commit `Cargo.lock`.** A caret range would let a patch release move
under a deployment, and the measured behaviour this project is built around belongs to one specific
release. The lockfile is committed because both artefacts are binaries, and because with a tree this
size it is the only practical record of what was actually built.

**Build like any other Rust project, ship and run the serving half in a container.** The image
carries both binaries. Its runtime base must match the build image's distro: `rust:1.96-slim` is
Debian 13, and a bookworm runtime rejects the binaries with a glibc error. The build also needs
`VERGEN_IDEMPOTENT=1`, because crates in this tree read git metadata at build time and there is no
checkout to read from inside an image layer.

## Consequences

- Upgrading the SDK is a deliberate, reviewable change, and the measurements in this repository can
  always be attributed to an exact tree.
- Whoever bumps the pin should re-run the measurement before trusting the old numbers: the failure
  rates recorded here are properties of a version and a network, not of the design.
- A container gives the serving half's identity a volume of its own and keeps the cover traffic
  attributable to one process. It does not make the dependency tree smaller, and is not claimed to.
- The dialling half is deliberately identity-free, so it has nothing to persist and can run wherever
  is convenient.
