# LAN exposure: app HTTP, DNS, Operator API + API key

On the Host, the Platform publishes LAN port **80** (Traefik, Consumer traffic) and **53** (dnsmasq). Applications do not publish host ports directly — traffic enters by Hostname. The **Operator HTTP API** also listens on the LAN so the CLI (and a future web console) can run on another device. Auth: **API key** generated on `init`, stored in local CLI config (Disco-style). mTLS and arbitrary Application port publish are out of MVP.

The “API = gRPC” binding is **superseded by ADR-0007**; Operator API exposure on the LAN remains.

**Status:** accepted

The blanket exclusion of Application port publishing is superseded for the
Compose MVP by [ADR-0014](0014-compose-applications-and-future-native-supervision.md).
Its supported port mappings and LAN exposure must be documented during implementation.

Who binds these ports is settled by [ADR-0017](0017-host-native-dns.md) for 53
and [ADR-0019](0019-embedded-http-proxy.md) for 80 and 443: the Platform
itself, on the Host, rather than a container it starts. ADR-0019 also adds a
port an Application answers on — its Web Target's, bound to loopback, which is
the proxy's way in and not LAN exposure.
