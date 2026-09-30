# Runtime and Publication are recorded on the Application

An Application's row says what it runs (an image or a Compose file) and where
it answers (Hostname, aliases, Web Target, Host port), and every reader took
two things for granted: Docker runs it, and a Hostname reaches it. Two things
coming break one assumption each. Native Applications, which
[ADR-0014](0014-compose-applications-and-future-native-supervision.md)
deferred, do not run in Docker. A worker or a database that another
Application talks to has no Hostname to answer on. The row now says both
things instead of implying them: `runtime` is `container` or `native`, and
`publication` is `web` or `unpublished`. A third field, `variable_delivery`,
records how the Application's Variables reach a Compose project; ADR-0030
decides what it means.

## What the fields mean

A row written before the fields existed reads as `container`, `web` and
`broadcast`. Nothing migrates and no reader changes for an old row (ADR-0027:
a new optional field reads old bodies). New Applications read `referenced`
for their Variables.

An unpublished Application has an empty Hostname, no aliases, no Web Target
and no Host port. It has no Record in the Zone, so its name is free for the
Operator's own Records, and no route in the proxy, so nothing on the LAN
finds it by name. A request that names a Hostname, an alias or a Web Target
for one is refused with `400` rather than recorded and ignored. A restart
never gives it a Host port, where it gives one to a published Application
deployed before the proxy moved into the binary (ADR-0019).

A native Runtime is a recorded shape only: the account name, the command, the
working directory, the loopback port and the resource limits. A create or
update that asks for it is refused with `501` and records nothing, until the
lifecycle slice that runs it lands. The shape exists now so the row, the task
queue and the API agree on it before anything depends on it.

Publication is chosen at creation. An update that asks for the other kind is
refused with `409`; asking for the same kind changes nothing. Switching a
published Application to unpublished means withdrawing a Hostname Consumers
may have bookmarked, and the other direction means allocating a port and
resolving a Web Target on a definition that never had one. Both are a later
decision, not a field write.

## Considered options

- **Nest the Hostname, aliases, Web Target and Host port under
  `Publication::Web`.** The type would carry the invariant. Rejected for this
  wave: routes, the DNS inventory, the console, the CLI, the terminal and the
  metrics all read the flat fields, and the ripple through each of them buys
  nothing the deploy path cannot enforce today.
- **A discriminator beside the flat fields, with the deploy path holding the
  invariant.** Chosen. The invariant lives in `apps.rs` and its tests rather
  than in the type. Nesting stays open once the readers have settled.

## Consequences

`DeployApplicationRequest` and `UpdateApplicationRequest` take optional
`runtime`, `publication` and `variable_delivery`. `ApplicationResponse`
always carries all three, so an old client keeps working and a new one can
tell a row that predates the fields from one that chose the defaults.

`routes::hostnames` answers with an empty list for an unpublished
Application, and `RouteStore::publish` withdraws its id instead of publishing
it. The DNS inventory derives from `routes::hostnames`, so it needs no
change of its own.

Web Target widens in `CONTEXT.md`: a native process's loopback port is one
too.

**Status:** accepted

**Extends:** [ADR-0014](0014-compose-applications-and-future-native-supervision.md)
(the native path it deferred now has a recorded shape),
[ADR-0015](0015-compose-service-ownership-and-routing.md) (a Compose
Application may have no Web Target),
[ADR-0019](0019-embedded-http-proxy.md) (an unpublished Application is never
in the route table, where a published one without a port answers `503`).
**Documented in:** [deployment-contracts.md](../deployment-contracts.md).
