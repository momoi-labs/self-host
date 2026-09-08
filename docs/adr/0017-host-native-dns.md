# The Platform serves DNS natively on the Host

The Platform binds UDP and TCP port 53 on the Host's configured LAN address.
CoreDNS no longer runs in a container. On macOS, putting DNS behind the Docker
VM required address forwarding and file mounts before the house could resolve
an Application Hostname. Native DNS removes those dependencies and keeps name
resolution available while Docker is starting or unavailable.

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
state store; `self-host setup-dns` reads it too, so the Host resolver agrees
with what the Platform actually serves. The Host needs a fixed LAN address; if
that address has not appeared at boot, DNS waits for it. Port conflicts or missing bind permissions
stop startup with an error so the supervisor can report and restart the daemon.

Port 53 is privileged, and the two Hosts grant it differently, so they do not
listen on the same address.

On Linux, the installer grants `CAP_NET_BIND_SERVICE` to the binary. An
Operator-managed systemd system unit can instead set
`AmbientCapabilities=CAP_NET_BIND_SERVICE` under `[Service]`. Binding only the
LAN address leaves systemd-resolved's loopback listener available.

macOS has no capabilities, and the LaunchDaemon of ADR-0013 runs as the
Operator so the Platform finds the same Docker context, configuration
directory and Compose projects the Operator sees. A non-root process there
cannot bind a named address below port 1024 — but it can bind the unspecified
one, because `in_pcbbind` applies the check only when the address is set. The
Mac therefore listens on every interface. The Platform stays out of root and
the files it writes keep belonging to the Operator, which is the point of
ADR-0013; running as root and dropping privileges after the bind, or handing
launchd the sockets, would each buy a named address at the price of code that
exists for one Host. The cost is that nothing else on the Mac can take port
53, loopback included, and that a reply to a query may leave from an address
other than the one queried if the Host is multi-homed.

Records come from the Host IP in `dns.json` on both, whatever the listener is
bound to.

Only PostgreSQL and Traefik remained Infra containers;
[ADR-0018](0018-platform-state-in-files.md) has since removed PostgreSQL.
Existing installations must be reset before using this version; no CoreDNS
migration is provided.

**Status:** accepted

**Supersedes:** the container DNS choice in [ADR-0003](0003-dnsmasq-traefik.md).
**Amends:** DNS port ownership in [ADR-0006](0006-lan-ports-http-dns-grpc.md) and
the Infra container list in [ADR-0011](0011-infra-containers-are-recreated-not-renamed.md).
**Context:** [issue #59](https://github.com/momoi-labs/self-host/issues/59) and
[ADR-0013](0013-macos-bootstrap-with-launchd.md). The macOS listen address was
settled by [issue #64](https://github.com/momoi-labs/self-host/issues/64),
after the first version of this decision assumed only Linux needed a privilege
story.
