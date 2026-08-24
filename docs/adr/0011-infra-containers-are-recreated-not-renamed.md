# Platform Infra containers take the `sf-system-` prefix by being recreated

The three Platform Infra containers are named `sf-system-db`, `sf-system-dns`
and `sf-system-proxy`. The middle segment names the role the component plays
for the Platform, not the product filling it: the state store is `db` even
though the image is `postgres:18-alpine`, so replacing the product is not a
rename.

ADR-0008 set this convention and left Infra out of it, because changing the
constants on a live install would have started a second Postgres competing for
the same port against the same volume, and no `docker rename` existed to adopt
the old container. This ADR closes that gap without the rename: the Platform
has no published release, so there is no install whose Infra must survive the
change. The old containers are destroyed and the new ones created in their
place.

`docker compose up` now runs with `--remove-orphans`. Renaming a service leaves
its old container behind, still holding the ports the new one needs; the
Platform owns `~/.config/self-host/docker-compose.yml` entirely, so a container
the file no longer lists has no reason to still be running. Applications are
started with `docker run` and carry none of the compose project's labels, so
they are never orphans of it.

Compose services now also set `container_name`. Without it the runtime name is
`<project>-<service>-1`, so the name promised here would not be the name
`docker ps` prints.

## Consequences

The `self-host-pg-data` volume keeps its name, as ADR-0008 decided: renaming a
volume means copying the data.

Anyone running an install from before this decision loses their Platform Infra
state — the Postgres container is recreated against the same volume, so the
Applications table survives, but the containers themselves do not. That is
acceptable exactly once, while nothing is published.

The old `self-host` bridge is left behind empty; `--remove-orphans` removes
containers, not networks. `docker network rm self-host` clears it by hand.

**Status:** accepted
**Amends:** Platform Infra naming left open in ADR-0008
