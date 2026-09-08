# Application routing lives in the Traefik file provider, and a Hostname can have aliases

An Application's Traefik router is written as a file in Traefik's dynamic
configuration directory — `traefik-dynamic/app-<id>.yml` — instead of as Docker
labels on the Application's container. The Application Hostname is owned by the
Operator: defaulted to `<name>.<suffix>` when the Application is created, and
editable afterwards. An Application may also carry **aliases**: additional
Hostnames the same Application answers on.

ADR-0008 made renaming an Application free by keying the container and the
router on a surrogate id. Changing the *Hostname* stayed expensive, because the
Host rule lived in a Docker label and Docker cannot rewrite a label on a
running container — so a cosmetic URL change destroyed and recreated the
container. Traefik already watched a file provider directory (it serves the
admin route), so moving the router there costs one file write and removes the
container from the equation entirely: Traefik reloads on its own, and the
container never learns that its URL changed.

Aliases exist because that is what makes a Hostname change *non-breaking*. With
routing decoupled from the container, ``Host(`a`) || Host(`b`)`` is one more line
in the same file, so an Operator renaming `blog.home.lan` to `writing.home.lan`
can keep the old Hostname resolving instead of breaking every bookmark on the
LAN. An alias is always explicit — a rename never adds one silently.

## Considered options

- **Keep labels, decide only the ownership policy.** Rejected: it answers the
  question on paper and leaves "changing a Hostname means downtime" true
  forever, which is the part that actually hurts.
- **A Traefik KV provider (Redis/Consul).** Rejected: another Infra container
  for a single-Operator home lab, to do what a watched directory already does.
- **File provider.** Chosen.

## Consequences

Nothing has to resolve or route differently for a new Hostname to work: CoreDNS
already answers `*.<suffix>` from a wildcard template, and the TLS certificate
is already a wildcard for the same suffix. Routing was the only part coupled to
the container.

The Application container no longer carries `traefik.*` labels — only
`sf.app.id` and `sf.app.name`, so it stays identifiable in `docker ps`. The
Docker provider stays enabled in Traefik's configuration and simply finds
nothing to do, since `exposedByDefault` is false. Containers deployed before
this ADR keep their old labels until the next deploy recreates them; while both
exist, the label router and the file router point at the same container and the
same port, so the overlap is invisible in traffic.

Route files are rewritten for every running Application at daemon start. The
dynamic directory is therefore a projection of the database, not a source of
truth: deleting it is repaired by a restart, and hand-editing a generated file
is not.

A Hostname change no longer recreates the container, so it is no longer
downtime and the console does not need to present it as such. Changing the
image still recreates.

**Status:** superseded

**Superseded by:** [ADR-0019](0019-embedded-http-proxy.md). The routes moved
from a directory Traefik watched to a table the Platform serves from
directly. What this decision was for survives it: routing is still keyed on
the Application's id rather than its container, a Hostname change is still a
write and not a recreate, and aliases still make a rename non-breaking.
**Extends:** ADR-0003 (Traefik with the Docker provider), ADR-0008
