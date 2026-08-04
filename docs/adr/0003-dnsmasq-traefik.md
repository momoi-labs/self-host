# DNS via dnsmasq + Traefik proxy

For the MVP, LAN clients must resolve `name.<suffix>` to the Host IP; HTTP routing by `Host` stays in the reverse proxy. Research in `docs/research/lan-dns-service-discovery.md` showed dumb DNS is enough and Consul is overkill. We chose **dnsmasq** as an Infra container (smaller surface than CoreDNS/Consul; less risk than implementing DNS in the binary now) and **Traefik** with the Docker provider. Consul and in-binary DNS are out of MVP.

**Status:** accepted
