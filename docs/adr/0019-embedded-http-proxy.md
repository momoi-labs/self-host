# The Platform terminates HTTP/HTTPS itself, without Traefik

The Platform binds ports 80 and 443 on the Host directly. Traefik no longer
runs. Publishing Applications now works the same way native DNS (ADR-0017)
already does: a maintained library embedded in the binary, not a container the
Platform depends on to reach its own console and API.

The proxy is built on `hyper` and `tokio-rustls`, both already pulled in
transitively through `axum` and `reqwest`. `rustls::ServerConfig` loads the
same `cert.pem`/`key.pem` pair `tls.rs` already generates and validates;
nothing about certificate generation, the local CA, or the trust workflow
changes. An HTTP listener on 80 does nothing but redirect to `https://`, as
Traefik's did. Reverse-proxying to an Application uses `hyper`'s client
directly rather than a crate built for it: the request/response types, chunked
bodies, and `Upgrade` handling for WebSockets are `hyper`'s, not
hand-rolled. ALPN offers HTTP/2 and HTTP/1.1, as Traefik did, and an
Application is reached over HTTP/1.1 either way — so a Consumer keeps the
protocol it had, and an Application sees the one it always saw. A request that
arrives on HTTP/2 carries its Hostname as `:authority` and no `Host` header,
so one is put back before it is forwarded; an Application that builds URLs
from `Host` would otherwise answer with links to a loopback port.
`admin.<suffix>` is not a proxied hop at all — the request is
served straight from the in-process console/API `Router` that `build_app`
already returns, since it and the proxy now share a process.

Application routing moves from a directory Traefik watches
([ADR-0009](0009-file-provider-routing-and-hostname-aliases.md)) to an
in-memory table the Platform updates directly: `publish`/`withdraw` write to a
`HashMap` behind a lock instead of a YAML file behind a filesystem watch.
Hostname and alias changes still take effect without recreating a container,
for the same reason they did under Traefik — the router was never keyed to
the container, only to the Application's id.

The harder change is what a route points at. Traefik and the container it
routed to shared a Docker bridge, so `http://sf-app-<id>:80` was reachable
from the router's own network namespace. The embedded proxy runs on the Host,
not on that bridge, and a container name resolves nowhere there — most
sharply on macOS, where Docker Desktop's bridge lives inside a VM the Host
process cannot reach at all. Each Application's Web Target is instead
published on a Host port bound to loopback, which Linux and macOS resolve the
same way from the Host.

The Platform chooses that port rather than asking Docker for an ephemeral one
and reading back what it got. The ports already handed out are on the records
the Platform holds, so the catalogue costs nothing to consult; a candidate is
checked against the Host as well, because a port some other program holds is
one the records cannot know about. Choosing it means the address is known
before the container exists and is written down with the Application, so a
redeploy answers at the same place and a restart reads the port back instead
of interrogating Docker for it. Between the check and the container that
publishes it there is a gap nothing can close: a lost race surfaces as that
deploy failing on that port, which is legible, rather than as a route to a
socket nobody is listening on.

Only the Web Target's service is published this way, alongside whatever ports
the Operator published themselves. The publication interface takes an
Application's identity, its Hostnames and that address — never a container
name or a Compose service, which stay on the execution side of it. An
Application with no port yet publishes with no address, so the proxy answers
`503` rather than guessing; an unknown Hostname was never in the table and
answers `404`, never falling through to the admin API or another Application.

## Consequences

Restarting the Platform binary now drops every Application's HTTP/HTTPS at
once, even one whose containers keep running: the proxy and the API share the
daemon's process and its failure boundary, where Traefik previously survived
a Platform restart on its own. This is accepted as the cost of removing a
container the Platform no longer needs to run at all. Failing to bind ends the
daemon, because a Platform that is up with no console, no HTTPS API and no
Application reachable by Hostname is of less use to a supervisor than one it
can restart.

Ports 80 and 443 are privileged, and the two Hosts grant them exactly as they
grant 53 to DNS ([ADR-0017](0017-host-native-dns.md)): `CAP_NET_BIND_SERVICE`
on Linux, granted to the binary by the installer, and on macOS the unspecified
address, which a non-root process may bind below 1024 where a named one it may
not. Binding the unspecified address on both Hosts keeps one listener rather
than one per Host, and costs the Mac the same thing DNS already costs it —
nothing else there may hold those ports.

Nothing of the Platform's runs in a container any more. `/system` lists no
Infra, no Application name is reserved, and the `sf-system` bridge has no
members; an installation that has one can remove it by hand.

An upgrade releases the ports before binding them: the proxy container is
removed, along with the generated Compose file that would hand them back on
the next `docker compose up`, and the configuration written for a program this
Host no longer runs. None of it is Application data and none of it is state.
An Application deployed before this has no Host port, which no running
container can be given, so startup recreates its workload once to publish
one — volumes, data and the record survive that, and a Host whose Docker is
unreachable migrates nothing and says so.

**Status:** accepted

**Supersedes:** the Traefik choice in [ADR-0003](0003-dnsmasq-traefik.md), the
file-provider mechanism in
[ADR-0009](0009-file-provider-routing-and-hostname-aliases.md), and the
two-bridge split in
[ADR-0012](0012-system-and-application-networks-are-separate.md) — with
nothing of the Platform's in a container, one network is left.
**Amends:** the LAN port ownership in
[ADR-0006](0006-lan-ports-http-dns-grpc.md) and the Infra container list in
[ADR-0011](0011-infra-containers-are-recreated-not-renamed.md), which is now
empty.
**Context:** [issue #67](https://github.com/momoi-labs/self-host/issues/67),
[A self-contained Platform](https://github.com/momoi-labs/self-host/blob/f1b3025aba2e76a675df0e06f5e8277288cdc3fd/docs/research/self-contained-platform.md),
and [ADR-0017](0017-host-native-dns.md).
