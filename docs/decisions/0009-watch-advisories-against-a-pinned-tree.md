# 0009. Watch advisories against a pinned tree

## Context

ADR 0002 pins `nym-sdk` exactly and makes upgrading a deliberate, re-measured change. The cost of
that decision is that advisories against the pinned tree cannot be fixed by routine updates: at the
time of writing, `cargo audit` reports eleven, all transitive through the SDK, none with a patched
version reachable from `=1.21.5-rc.3`. That pin is also the newest release on crates.io, so there is
no upgrade to fall back on today.

Publishing the repository in that state without a stated policy leaves two problems open: reporters
have no channel to raise a vulnerability, and a *new* advisory landing on top of the eleven known ones
would look identical to them and go unnoticed.

## Decision

**Enumerate the known advisories with justifications in `.cargo/audit.toml`.** Each entry names the
crate and the advisory in one line; the list exists to be outgrown, not to be added to.

**Run `cargo audit` on a schedule in CI**, so it fails only on an advisory that isn't already in that
file — the known eleven pass, anything new breaks the build.

**Re-evaluate the list at every SDK bump.** Whoever bumps the pin drops the entries the new version
fixes and re-justifies whatever remains, rather than carrying the old list forward unread.

**Document a reporting channel in `SECURITY.md`.** Vulnerabilities specific to this project's own
code have a place to go that isn't a public issue.

## Consequences

- A new advisory against the pinned tree is noticed within a week of the next scheduled run, not
  whenever someone happens to run `cargo audit` by hand.
- The ignore list is a standing admission, not a backlog: it must shrink at every SDK bump, and
  growing between bumps is a sign something was silenced instead of justified.
- A critical advisory that lands in the mixnet crypto path is grounds to bump the SDK ahead of
  schedule, accepting ADR 0002's re-measurement cost rather than waiting out the ignore list.
