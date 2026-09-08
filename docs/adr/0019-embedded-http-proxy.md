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
hand-rolled. `admin.<suffix>` is not a proxied hop at all — the request is
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
published to a loopback address with an ephemeral host port
(`127.0.0.1:0` mapped to the container port), which Linux and macOS both
resolve the same way from the Host. The Docker adapter discovers the assigned
port after `run`, `compose up`, or a recreate, and reports it through a
publication interface that takes an Application's identity, its hostnames,
and that resolved target — not a container name or a Compose service, which
stay inside the Docker-specific side of that interface. An unresolved target
publishes with no address, so the proxy answers `503` instead of guessing at
a route; an unknown Hostname was never in the table and answers `404`, never
falling through to the admin API or another Application. A loopback
publication is never on top of an Operator's own explicit port mapping — one
already reachable is left alone.

## Consequences

Restarting the Platform binary now drops every Application's HTTP/HTTPS at
once, even one whose containers keep running: the proxy and the API share the
daemon's process and its failure boundary, where Traefik previously survived
a Platform restart on its own. This is accepted as the cost of removing a
container the Platform no longer needs to run at all.

Binding 80 and 443 on the Host hits the same non-root binding problem
[#64](https://github.com/momoi-labs/self-host/issues/64) already tracks for
port 53; that issue's fix is expected to cover the proxy's listeners too.

**Status:** accepted

**Supersedes:** the Traefik choice in [ADR-0003](0003-dnsmasq-traefik.md) and
the file-provider mechanism in
[ADR-0009](0009-file-provider-routing-and-hostname-aliases.md).
**Amends:** the LAN port ownership in
[ADR-0006](0006-lan-ports-http-dns-grpc.md) and the Infra container list in
[ADR-0011](0011-infra-containers-are-recreated-not-renamed.md), which drops to
one component.
**Context:** [issue #67](https://github.com/momoi-labs/self-host/issues/67),
[A self-contained Platform](https://github.com/momoi-labs/self-host/blob/f1b3025aba2e76a675df0e06f5e8277288cdc3fd/docs/research/self-contained-platform.md),
and [ADR-0017](0017-host-native-dns.md).
