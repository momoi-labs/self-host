# Container naming: `sf-` prefixes and a surrogate Application id

Containers the Platform owns are named `sf-app-<id>` for Applications and
`sf-system-<name>` for Platform Infra. The prefix says who owns the container,
so `docker ps` is readable and `docker rm` is survivable: anything under `sf-`
belongs to the Platform, and the middle segment says whether losing it costs a
user's app or a piece of the platform itself.

Applications carry a surrogate `id` — 12 characters of lowercase base32,
excluding the pairs that misread in a terminal (`l`/`1`, `o`/`0`). The
container and the Traefik router are named from that id, never from the
Application name.

That indirection exists so an Application can be renamed. When `name` was the
primary key, the container name and every Traefik router label were derived
from it, so a rename meant destroying and recreating the container — downtime
for a cosmetic change. With an id, a rename is a single row update and Docker
never learns about it. The name rides along as the `sf.app.name` label so the
container is still identifiable by eye.

An id is random rather than sequential: it does not leak how many Applications
exist, and it keeps its meaning across a dump and restore, which a `BIGSERIAL`
would not.

## Consequences

Existing rows migrate with `id = name`, so `sf-app-<id>` resolves to the
container that is already running under the old scheme and **no container is
recreated by the upgrade** — but only for Applications. Platform Infra
containers created before this ADR are still named `self-host-pg`,
`self-host-coredns` and `self-host-traefik`; renaming those safely needs a
`docker rename` step that does not exist yet, because simply changing the
constants would start a second Postgres competing for the same port. That
migration is tracked separately and is not done here.

The `self-host-pg-data` volume keeps its name. Renaming a volume means copying
the data, which is not worth it for consistency alone.

Changing an Application's hostname still recreates its container, because
Docker cannot rewrite labels on a running container. Only the rename case is
free.

**Status:** accepted
