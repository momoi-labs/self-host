# Platform state lives in files the Platform owns

The Platform keeps its authoritative state in `~/.config/self-host/state/`,
written by the daemon as JSON. PostgreSQL is gone: no Infra container, no image
pull, no database connection, no SQLx dependency.

Keeping state in a container made Docker a dependency of reading configuration.
A Host where Docker was missing, still booting or broken had no console, no
Application list and no API credentials either — the Operator could not even
see what was configured, let alone why it was not running. Docker executes
Applications. It was never the right place to keep what the Operator asked for.

## The commit unit is one file

`platform.json` holds the format version, the Platform settings and the
Operator's credentials. `state/applications/<id>/application.json` holds one
Application entirely: identity, publication, Web Target, environment, stopped
intent, pending operation and last error. Nothing the Platform owns spans two
files, so every mutation is a single atomic replace and there is no multi-file
commit protocol to recover from. Directories are named by the stable
Application ID, so a rename touches one file and no workload.

The Operator's Compose definition is the exception, and it is written the other
way round: the verbatim file lands first as `compose-<digest>.yaml`, and only
then does the record that references it commit. A Compose file nothing
references is work interrupted before it counted, and startup collects it,
along with a directory that never got a record.

Writes replace: the temporary file is flushed, renamed over the destination,
and the directory flushed afterwards. Rename makes the replacement atomic for a
reader; the directory flush is what makes it survive a power cut. macOS
acknowledges `fsync` before the drive does, so the Platform asks for
`F_FULLFSYNC` there. A filesystem that refuses to flush a directory is not
treated as a failure — there is nothing to flush — and the durability claim
does not hold on it.

State that does not parse, or that declares a newer format version, stops the
daemon with the path and the reason. It is never read as an empty installation.

## One writer

The daemon holds an advisory `flock` on `state/platform.lock` for as long as it
runs, and a second writer is refused by path rather than allowed to interleave.
Because the lock is held by the kernel, a daemon killed without cleanup does
not lock the Platform out of its own state. CLI and console mutations go
through the API, as they already did. Readers that only report configuration —
`self-host init` checking whether the Host is already bootstrapped — open the
directory without the lock and cannot write through that handle.

Name uniqueness is enforced at the commit, not only in validation above it.
PostgreSQL held it as a `UNIQUE` constraint, and two concurrent requests can
both pass a check that is not where the write happens.

## What is authoritative and what is not

`state/` is authoritative and nothing else is. Rendered Compose projects under
`apps/<id>/`, Traefik routing under `traefik-dynamic/` and Docker's own view of
the world are projections, rebuilt from committed records at startup. `dns.json`
is a projection too: Bootstrap derives it from the DNS Suffix and Host IP it
commits, and the daemon reads it before the store so DNS can answer first.
`self-host setup-dns` reads `dns.json` rather than the store, because the
running daemon holds the store and the Host resolver has to agree with what DNS
actually serves.

Directories are `0700` and files `0600`, including temporary files: the
Operator's credentials and every Application's environment are in there.

## Upgrading from PostgreSQL

The Platform cannot read the old database, and starting an empty installation
beside it would strand every Application the Operator deployed. `self-host init`
detects the `sf-system-db` container and refuses, naming the `pg_dump` that
exports it and the `reset` that clears the way. There is no automatic import;
see [operating the Platform](../operating.md).

**Status:** accepted

**Supersedes:** [ADR-0002](0002-platform-state-postgres-18.md).
**Amends:** the Infra container list in
[ADR-0011](0011-infra-containers-are-recreated-not-renamed.md) and the network
placement of the state store in
[ADR-0012](0012-system-and-application-networks-are-separate.md).
**Context:** [issue #66](https://github.com/momoi-labs/self-host/issues/66) and
[A self-contained Platform](../research/self-contained-platform.md).
