# A self-contained Platform

Date: 2026-09-07. Documentary research against the current code and official
documentation. No proxy benchmarks, power-loss tests or native supervision
experiments were performed for this note.

## Direction

Run the Platform from one binary and one authoritative data directory. Embed
DNS, HTTP/HTTPS publication, the API and the console. Replace PostgreSQL with
files. Keep Docker/Compose as the first Application executor, behind an interface
that can later support native Applications.

Bootstrap and daemon startup must work without Docker installed or running.
The Operator must still reach the console, read configuration, make edits that
do not require execution, and see why Application operations are unavailable.
Edits that currently recreate workloads retain their execution requirements.
Docker availability belongs to the
execution path. The combined PostgreSQL and Traefik replacements deliver this
outcome; removing only one container does not.

The API keeps its current routes, payloads, status codes, authentication and
synchronous behavior. Its storage implementation reads and writes disk files
behind the existing StateStore interface. CLI and console contracts stay intact.

This removes separately installed Platform services. Rust libraries and the
Host's normal startup mechanism remain. Native Applications and installing s6
are outside these two implementation slices.

## What exists today

- [StateStore and PgStateStore](../../src/db.rs) hold Platform settings,
  credentials and API-key metadata, Application records, Compose source,
  environment variables and operation errors. PostgreSQL is more than an index
  of running containers.
- [Daemon startup](../../src/main.rs) starts embedded DNS but waits for Docker
  and PostgreSQL before serving the API. The
  [console bundle](../../src/console.rs) is already embedded.
- [Bootstrap](../../src/bootstrap.rs) prepares Platform containers and TLS.
  [TLS configuration](../../src/tls.rs) configures Traefik for HTTPS, HTTP
  redirection and the admin hostname.
- [Application routes](../../src/routes.rs) are derived Traefik files. Their
  upstreams use container names and the selected Web Target port.
- [Application operations](../../src/apps.rs) persist a pending operation before
  execution and reconcile at startup. [DockerRuntime](../../src/docker.rs) is an
  existing test boundary, but its container and Compose methods are specific to
  Docker.

## Files as the state store

Keep three categories distinct. Desired configuration records what the Operator
asked for. Observation reports what the executor can currently verify. Operation
records preserve interrupted work and the last error. An unavailable Docker
daemon must not turn an unknown observation into a stopped Application.

An illustrative layout follows. File names and transaction boundaries are still
implementation choices, not an accepted storage schema.

```text
<data-dir>/
|-- platform.json                 # format version, DNS Suffix, Host settings
|-- auth.json                     # credentials and API-key metadata
|-- tls/                          # certificates, CA and private keys
|-- applications/
|   `-- <application-id>/
|       |-- application.json      # identity, publication, executor, desired state
|       |-- compose.yaml          # original Operator definition for Compose
|       |-- env.json              # Application environment, including secrets
|       `-- operation.json        # pending work and last result/error
`-- generated/                    # disposable executor configuration
```

Use stable Application IDs for paths. A rename must preserve identity and
workload ownership. Original Compose source and environment are authoritative;
rendered Compose projects and routing tables can be rebuilt. A future s6 service
directory would also be derived configuration.

The recommendation is one daemon writer, with CLI and console mutations going
through its API. Offline Bootstrap or import must acquire the same exclusive
store lock. A second process must fail clearly rather than write concurrently.
Serialize validation and commit so concurrent requests cannot claim the same
Application name or conflicting hostname. Rebuild lookup indexes from committed
records unless measured need justifies persisting them.

Durability needs an explicit protocol. For a single file on Linux, write a
temporary file in the destination directory, flush it, replace the destination
with rename, then flush the parent directory before acknowledging success.
Rename makes replacement atomic for readers; it does not by itself establish
power-loss durability. Linux documents directory synchronization separately.
[rename(2)](https://man7.org/linux/man-pages/man2/rename.2.html),
[fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html).

Several atomic renames do not make one atomic Application update. Choose a small
commit boundary before fixing the layout, for example a single record containing
related fields, or immutable revisions selected by one committed pointer.
Define recovery for creation, updates and deletion, including orphaned files.
SQLite's commit documentation illustrates why multi-file commits need their own
protocol; it is background on durability, not a proposal to adopt SQLite.
[SQLite atomic commit](https://www.sqlite.org/atomiccommit.html).

Record operation intent before external side effects. After a crash, inspect the
executor and finish or report interrupted work without duplicating workloads.
Preserve an explicit stopped intention across restart. Do not introduce a
continuous reconciliation loop or a new asynchronous API solely for this change.

Use restrictive ownership and permissions, such as directories `0700` and secret
files `0600`, including temporary files and generated files containing secrets.
Validate the format version and report corrupt records with their paths. Never
silently replace unreadable state with an empty installation. Verify the chosen
durability protocol on supported Linux and macOS filesystems before promising
power-loss recovery on both.

A consistent backup of this directory preserves Platform state and credentials.
Take it with the writer stopped or through a defined snapshot/export operation.
It does not back up Docker volumes or Application bind mounts. Restore must
preserve IDs, credentials and the CA, then rebuild derived configuration.

## Execution and publication interfaces

Follow the existing [Application definition](../../CONTEXT.md), which permits
multiple services and an identity independent of execution. Define execution at
the Application level: prepare/start, stop, restart, remove, observe, stream logs
and resolve a reachable Web Target. Keep per-service observations and logs for
Compose Applications. Exact method signatures should follow current callers.

Docker/Compose implements this boundary first. Its definition stays Compose;
avoid inventing a universal manifest or disguising container operations as a
generic executor. Storage owns persistence. Publication accepts hostnames and an
endpoint supplied by execution, without constructing Docker names or inspecting
Compose itself. These boundaries should allow a later native executor without
rewriting storage, DNS or the proxy.

Moving the proxy onto the Host changes network reachability. Docker Desktop's
bridge network is inside its VM, and published ports are its documented route
from the Host to containers. Existing container-name URLs cannot simply be
copied into an embedded proxy.
[Docker Desktop networking](https://docs.docker.com/desktop/features/networking/).

Loopback-published Web Target ports are a candidate shared approach for Linux
and macOS. The Docker adapter must allocate/discover the binding and refresh the
endpoint after recreation. Validate access restrictions with the supported Docker
versions and network configuration; do not accidentally publish these ports to
the LAN. Preserve explicit Operator port mappings and Platform Infra isolation.
[Docker port publishing](https://docs.docker.com/engine/network/port-publishing/).

Use maintained HTTP/TLS libraries inside the binary. This research does not
select a proxy crate or establish its compatibility. Verify hostname and alias
routing, admin routing, certificates, HTTP redirects, streaming, WebSockets,
forwarded headers, disconnects and unavailable upstreams before replacing
Traefik. Route updates should swap a validated routing table without restarting
Applications. The daemon's shared failure boundary is a tradeoff of embedding
the proxy and API in the same process.

## What to retain from the s6 research

[PR #41](https://github.com/momoi-labs/self-host/pull/41), originating in
[issue #40](https://github.com/momoi-labs/self-host/issues/40), contains research
and proposed implementation work. Its
[controller model](https://github.com/momoi-labs/self-host/blob/789542fe93988b36ff711c39dc79f365f71b1c8c/docs/research/controller-model.md)
separates intent, observation and execution. Its
[s6 proposal](https://github.com/momoi-labs/self-host/blob/789542fe93988b36ff711c39dc79f365f71b1c8c/docs/research/s6-process-runtime.md)
treats service directories as generated output. Both ideas remain useful with
files replacing PostgreSQL and a proxy replacing Traefik.

Those documents retain PostgreSQL and Traefik and propose installing s6. They
provide no executable prototype or recorded Linux/macOS validation. Their
single-container examples need adaptation to today's Compose Applications.
They also disagree on when desired state lands and combine a zero-action
unchanged pass with unconditional rewriting of derived files. Inspecting for
drift does not require rewriting unchanged output.

The example changes directories before starting `s6-notifyoncheck`, whose
documented contract requires starting in the service directory. A future
prototype must move the Application's directory change into the child command
and validate readiness.
[s6-notifyoncheck](https://skarnet.org/software/s6/s6-notifyoncheck.html).

s6 supervises separate processes through its own programs. It does not supervise
individual modules inside the Platform process.
[s6 overview](https://skarnet.org/software/s6/overview.html).
Keep it deferred under [ADR-0014](../adr/0014-compose-applications-and-future-native-supervision.md).
The existing [Platform supervision research](self-host-supervision.md) remains
relevant to boot and daemon recovery. These replacements do not implement a
native executor or revise that supervision decision.

## Bounded implementation slices

1. [Replace PostgreSQL with file persistence (#66)](https://github.com/momoi-labs/self-host/issues/66)
   behind the existing StateStore interface. Specify the format,
   commit/recovery protocol and locking; preserve all current records and API
   behavior. Remove PostgreSQL provisioning and startup waits, and decouple the
   internal API listener from Docker availability. Choose an explicit migration
   policy: validated import, or a documented reset-required transition that
   preserves the old data for recovery. Never silently discard existing state or
   delete its volume automatically. Traefik may remain during this intermediate
   slice, so public HTTPS still depends on it.
2. [Replace Traefik with an embedded proxy (#67)](https://github.com/momoi-labs/self-host/issues/67).
   Introduce the Application
   execution boundary and Host-reachable Docker endpoints needed by the proxy.
   Remove Traefik provisioning/configuration and complete Docker-free startup
   for public HTTP/HTTPS.
   Define a controlled port 80/443 handoff for existing installations, preserving
   certificates and routes, with rollback if the replacement cannot listen.

The first slice must test restart persistence, concurrent uniqueness checks,
interrupted writes, corrupt/versioned data, stopped intent, secrets permissions
and the chosen migration policy plus restore of all stored entities. The second
must test actual HTTP/HTTPS
and WebSocket/streaming behavior, route updates, Compose multi-service lifecycle,
endpoint changes, and existing-installation cutover on Linux and macOS.

Together they must demonstrate fresh Bootstrap and an accessible authenticated
console/API with PostgreSQL, Traefik, Docker and s6 absent. Repeat startup with
stored Applications and Docker unavailable; show configuration and a specific
execution error without blocking the Platform. Then enable Docker and verify
Application operation and publication. Host port permissions, DNS integration
and certificate trust remain explicit installation requirements.
