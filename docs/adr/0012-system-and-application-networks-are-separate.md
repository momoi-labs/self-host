# Platform Infra and Applications sit on separate networks

There are two bridges: `sf-system` for Platform Infra and `sf-apps` for
Applications. Traefik is on both. Nothing else crosses.

Until now every Application shared one bridge with the state store, whose
credentials are fixed at `selfhost:selfhost` on every install. Any Application
an Operator published — including an image they did not write — could open a
socket on `sf-system-db:5432` and read every Operator secret the Platform
keeps. Separating the bridges puts the state store out of reach.

Nothing legitimate is lost by the split. CoreDNS
answers Consumers on the Host's port 53, and Applications are reached by
Traefik through the file provider (`http://sf-app-<id>:80`), which resolves
over the bridge Traefik shares with them.

## Consequences

Traefik is the one component on two networks, which matches what it is: the
only thing that must talk to both sides.

The network is named for who lives on it, matching the container prefixes from
ADR-0008 and ADR-0011, so `docker network inspect sf-apps` lists exactly the
Applications.

[ADR-0018](0018-platform-state-in-files.md) removed the state store from
`sf-system` entirely: Platform state is files the binary owns, so there is no
longer a database on any bridge for an Application to reach. The split still
holds for whatever Platform Infra remains.

Changing a network name recreates every container attached to it. That is free
today for the same reason ADR-0011 gives — nothing is published — and would not
be later.

**Status:** superseded

**Superseded by:** [ADR-0019](0019-embedded-http-proxy.md), which leaves one
network. Nothing of the Platform's runs in a container to need `sf-system`:
the proxy is on the Host and reaches an Application through a loopback port,
so no component crosses into `sf-apps` at all. What this decision protected is
stronger for it — an Application shares a bridge with other Applications and
with nothing of the Platform's. An `sf-system` bridge left behind by an older
install has no members; `docker network rm sf-system` clears it.
**Amends:** the single `self-host` network implied by ADR-0003
