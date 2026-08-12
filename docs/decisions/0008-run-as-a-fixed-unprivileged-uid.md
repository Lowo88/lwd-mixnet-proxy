# 0008. Run as a fixed unprivileged uid

## Context

The image ran as root, which is the default and was never a decision. Neither half needs the
privilege: they bind ports well above 1024 and write nothing outside the serving half's state
directory. Both are also long-lived processes that speak to an anonymising network and, on the
serving side, to whatever an operator points them at.

The state directory is the complication. It holds the private keys the serving half's address is
derived from, it lives in a volume, and the ownership of a volume is decided once, when it is first
initialised from the image.

## Decision

**Run as uid and gid 10001, declared numerically.** `USER 10001:10001` rather than a name, so the
identity survives an image whose `/etc/passwd` says something else, and so a runtime that insists on
a non-root uid can see it without resolving anything.

**Create `/state` in the image, owned by that uid and mode 700.** An empty named volume inherits
both, so the common path works with no extra step and the key directory is not world-readable.

**Treat the number as part of the interface.** It is documented rather than left to whatever the base
image hands out, because an operator bind-mounting a host directory has to `chown` it to the same
number.

## Consequences

- The compose file in this repository works as it stands: its named volume is initialised from the
  image and lands owned by 10001.
- A bind mount does not inherit anything and has to be chowned first, which is a step someone will
  forget. The failure is loud, and it happens at startup.
- A volume created by an earlier root-running image stays owned by root and the process can no longer
  write to it. Discarding it is the fix, and it costs the identity: the address changes, and anyone
  who wrote the old one down cannot reach that half again.
- Binding a port below 1024 now needs a capability the container does not have. The defaults are all
  above it, and publishing on a privileged host port is the runtime's job, not the process's.
