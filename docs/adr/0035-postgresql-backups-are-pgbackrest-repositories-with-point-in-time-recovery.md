# PostgreSQL backups are pgBackRest repositories with point-in-time recovery

A managed PostgreSQL server with backups on keeps a pgBackRest repository in
the Host's S3 bucket: physical backups of the whole server and every WAL
segment since. A restore makes a new server at the latest state or at a time
the Operator picks.

The scope is one server on one Host: back up, restore, and later a major
upgrade. Replication and high availability belong to pg-yard.

## Why not logical dumps

The first design, built and never released, kept a `pg_dump` of each consumer
database per run. Its restore put one database back into the same server, at
the time of a backup. The Operator expected a restore to give a new server,
and wanted point-in-time recovery. A dump cannot do either, so pgBackRest is
the only method and the dump design is gone. The record keeps no `method`
field: a second method is not planned.

What that costs: a restore is always the whole server and always the same
PostgreSQL major. Moving one database, or moving to another major, is an
import.

## What a server with backups on is

- **Storage: S3 only, over HTTPS.** The Host has one backup storage:
  endpoint, region, bucket, prefix, an access key, and whether to verify the
  TLS certificate. pgBackRest has no plain HTTP mode, so an `http://`
  endpoint is refused. Certificate verification can be turned off for a home
  lab bucket with a self-signed certificate.
- **Repository: one per server**, at `<prefix>/<database id>/`. The id, not
  the name, so a restored copy never shares a path with its source.
- **The image is a custom image.** `archive_command` runs inside the database
  container, so pgBackRest must be in the image. A package on the Host is out
  of the container's reach, and on a Mac it is a macOS binary. The Platform
  makes one custom image per PostgreSQL major, `postgres-<major>-backups`:
  the official `postgres:<major>-bookworm` plus the `pgbackrest` package from
  the PostgreSQL apt repository the official image already uses. The Custom
  images feature builds it on the Host and shows its log (ADR-0022). The
  record is marked as the Platform's, and the Operator cannot edit it. A
  server without backups keeps the official image.
- **A server created from a custom image keeps it.** The Operator picks a
  custom image at creation to add extensions. Swapping it for
  `postgres-<major>-backups` would drop those extensions, so the Operator's
  image must carry pgBackRest itself. The "PostgreSQL" custom image template
  installs it. Turning backups on checks for the binary and, when it is
  missing, fails with the lines to add. The Platform never edits the
  Operator's Dockerfile.
- **Configuration is a mounted file.** The Platform writes `pgbackrest.conf`,
  with the S3 keys, under a directory of its private data directory that only
  the Platform's user can enter, and mounts it read-only into the container.
  The file itself is mode 0644: the container's `postgres` user, which is not
  the Platform's, must read it. The secret never enters a Task
  payload, process arguments, an audit event or an API response, and a
  change of storage needs no restart.
- **No client-side encryption.** Encryption at rest is the bucket's.
- **WAL archiving is always on**, with `archive_timeout = 60`, so at most one
  minute of writes is outside the bucket.

Turning backups on recreates the container with the custom image, creates the
repository, turns archiving on with one restart, and takes the first full
backup. The repository comes first: PostgreSQL with archiving on and no
repository cannot shut down cleanly.

## When backups run

The Host has one **maintenance window**: a start hour in the Host's local
time and a length. A server with backups on gets a full backup on Sunday, a
differential on Wednesday and Saturday, and an incremental on the other days.
Due backups start at the window's start, one server after another, and none
starts after the window ends. A run the Host missed waits for the next
window. **Back up now** is always available and ignores the window. Every
backup is a Task on the server's own queue (ADR-0027).

**Retention is the recovery window.** The Operator sets days to keep, 14 by
default. pgBackRest keeps the full backups that cover those days and the WAL
between them, and expires the rest at the end of each backup. The Operator
does not delete single backups.

## When the bucket is unreachable

Archiving fails, PostgreSQL keeps the WAL and retries, and nothing is
dropped. The console shows the problem on the server and in the list of
databases. The WAL grows on the data volume until the bucket answers again,
so a long outage can fill the disk. The Operator chose a visible failure
over a silent gap in the recovery window.

## Restore and verify

Restore makes a **new managed PostgreSQL server** under a name the Operator
gives. A one-off container restores the latest backup, or the one before the
chosen time, into the new server's empty volume. The server starts, replays
WAL from the source's repository up to the target, and promotes. The
Platform then turns archiving off and detaches the server from the
repository, so it never writes into the source's history. The new server has the databases, roles and
passwords the source had at that time, and the Platform copies the source's
credential records to it. It has no connections and its backups are off. The
source is not touched.

**Verify** is on demand: restore the latest backup into a throwaway volume
and container with no published port, start PostgreSQL, query every
database, remove both. Nothing verifies on a schedule.

## Not decided here

- Restore over the source server in place.
- Asynchronous archiving and parallel backup tuning.
- What a major upgrade does with the repository.
