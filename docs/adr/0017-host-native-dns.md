# The Platform serves DNS natively on the Host

The Platform binds UDP and TCP port 53 on the Host's configured LAN address.
CoreDNS no longer runs in a container. On macOS, putting DNS behind the Docker
VM required address forwarding and file mounts before the house could resolve
an Application Hostname. Native DNS removes those dependencies and keeps name
resolution available while Docker and PostgreSQL are starting or unavailable.

Hickory serves an in-memory wildcard zone for the DNS Suffix and forwards other
names to Cloudflare at `1.1.1.1` and `1.0.0.1`. Its server and resolver handle
EDNS, UDP truncation, TCP fallback, caching and upstream failures. The Platform
does not implement DNS packet parsing or recursive resolution. Local address
records have a 60-second TTL; the Host address determines whether they are A
or AAAA records. Other local record types return no address data and never
leak to an upstream. A local adapter turns wildcard type misses into NODATA;
Hickory otherwise returns NXDOMAIN, which can invalidate a working address
record in client caches. The suffix apex holds SOA and NS records.

Bootstrap writes the DNS Suffix and Host IP to `dns.json` in the Platform
configuration directory. The daemon reads it before touching Docker or the
state store. The Host needs a fixed LAN address; if that address has not
appeared at boot, DNS waits for it. Port conflicts or missing bind permissions
stop startup with an error so the supervisor can report and restart the daemon.

On Linux, the installer grants `CAP_NET_BIND_SERVICE` to the binary. An
Operator-managed systemd system unit can instead set
`AmbientCapabilities=CAP_NET_BIND_SERVICE` under `[Service]`. Binding only the
LAN address leaves systemd-resolved's loopback listener available. macOS keeps
the LaunchDaemon selected in ADR-0013.

Only PostgreSQL and Traefik remain Infra containers. Existing installations
must be reset before using this version; no CoreDNS migration is provided.

**Status:** accepted

**Supersedes:** the container DNS choice in [ADR-0003](0003-dnsmasq-traefik.md).
**Amends:** DNS port ownership in [ADR-0006](0006-lan-ports-http-dns-grpc.md) and
the Infra container list in [ADR-0011](0011-infra-containers-are-recreated-not-renamed.md).
**Context:** [issue #59](https://github.com/momoi-labs/self-host/issues/59) and
[ADR-0013](0013-macos-bootstrap-with-launchd.md).
