# Compose services belong to the Application by id, and one of them answers the Hostname

ADR-0014 accepts Compose definitions and leaves two things to the
implementation: how the services of one Application are owned and named, and
how a Hostname reaches a project that has more than one container.

The Compose project is named after the Application's container prefix,
`sf-app-<id>`, and every service's container is `sf-app-<id>-<service>`. The
Operator's `container_name` is replaced. Identity labels carry the id, the
name and the service. A rename touches nothing in Docker, as ADR-0008
promised for one container; the same holds for five.

The Hostname routes to one service and one container port, the web target.
It is resolved when the definition is accepted, defaulting to the first
service that publishes a port and the container side of its first
publication, and recorded on the Application. The Traefik file-provider
route (ADR-0009) points at `sf-app-<id>-<service>:<port>` over `sf-apps`.

Services join the project's own network and `sf-apps`, never `sf-system`.
The Platform refuses a definition that names networks of its own, that asks
for a host port the Platform Infra uses, or that uses a key outside the
documented subset. Refusing by name is the contract: nothing is silently
dropped.

Persistent data is the Operator's, not the container's. `~` and relative
paths land under `~/.config/self-host/apps/<id>/data`; named volumes are
created by Compose under the project name. Removing an Application removes
its containers and network and keeps both.

The state the console shows is the row checked against Docker. A row that
says running with a container that has exited reads as failed, with the
service, exit code and restart count as the reason. `stopped` is the
Operator's word and survives a restart of the Platform or the Host.

**Status:** accepted

**Amends:** [ADR-0008](0008-container-naming-and-application-ids.md) for
Compose Applications; implements the mapping
[ADR-0014](0014-compose-applications-and-future-native-supervision.md)
required.

**Documented in:** [compose-applications.md](../compose-applications.md).
