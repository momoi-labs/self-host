# LAN DNS and service discovery for a self-hosted PaaS (MVP)

**Date:** 2026-08-03  
**Research question:** For a self-hosted PaaS on a single host on the LAN, with apps in Docker behind a reverse proxy keyed by `Host`, which DNS / service discovery approach meets the MVP (`name.<suffix>` → host IP) without depending on external SaaS — and what to leave for phase 2?

## How to read

- **Facts** come from official docs, man pages, RFCs, or first-party repositories; every factual claim has a citation.
- **Opinion / recommendation** is isolated in the [Recommendation](#recommendation-opinion) section.
- **Uncertainties** are marked explicitly.
- Container/compose sketches only use flags/commands documented by the cited sources; placeholders (`HOST_LAN_IP`, `paas.lan`) are from our scenario, not invented by the product.

### Project constraints (evaluation criteria)

| Constraint | Implication |
| --- | --- |
| A single Host on the LAN | No multi-node cluster in the MVP |
| Control-plane starts infra containers on demand | DNS can be a container managed by the platform |
| Apps = Docker containers | Routing can use Docker labels (Traefik) |
| Consumers on the LAN use HTTP | DNS must resolve from laptops/phones on the LAN, not only between containers |
| Local DNS **required** in the MVP | `name.<suffix>` → host IP; proxy routes by `Host` |
| No SaaS on the happy path | Self-hosted Consul OK; cloud DNS not |
| TLS out of MVP | HTTP only |
| Kubernetes out of scope | Ignored |

---

## Direct answers to the brief questions

### Is “dumb DNS” enough, or is full service discovery needed?

**For the described MVP — `*.paas.lan` (example) → host IP + reverse proxy by `Host` header — “dumb” DNS (wildcard / whole domain → one A record) is sufficient.**

Factual reason:

1. DNS only needs to deliver the address of the host where the proxy listens.
2. Traefik (Docker provider) builds routes from container labels, typically `Host(\`example.com\`)` — discovering *which container* serves the hostname is the proxy’s job, not DNS. Source: [Traefik Docker provider](https://doc.traefik.io/traefik/v3.5/reference/install-configuration/providers/docker/).
3. Full service discovery (catalog, health, SRV, multi-instance) solves a *different* problem: finding the address/port of healthy instances. In the “everything on the same host behind one HTTP proxy” model, that is redundant in the MVP.

### Can you start “dumb” and add Consul later without rewriting the model?

**Yes, if the platform model remains: app name → HTTP hostname → proxy on the host.**

- Phase 1: wildcard DNS → host IP; Traefik Docker provider with labels `Host(\`name.paas.lan\`)`.
- Phase 2 (optional): register services in Consul (`PUT /v1/agent/service/register`) and/or use Traefik’s `consulCatalog` provider. Sources: [Consul register services](https://developer.hashicorp.com/consul/docs/register/service/vm), [Traefik Consul Catalog](https://doc.traefik.io/traefik/reference/install-configuration/providers/hashicorp/consul-catalog/).

The external contract for LAN clients (`http://name.paas.lan`) can stay the same. What changes is *where* Traefik gets dynamic config (Docker labels → Consul tags), not the role of unicast DNS on the LAN.

**Uncertainty:** if in phase 2 DNS starts pointing at individual container IPs (instead of the host), the network/port-publishing model changes — that *would* be an architecture change, not just an implementation change.

---

## Option comparison

### Table 1 — Overview

| Option | Primary role | Wildcard `*.suffix` → 1 IP | Service catalog / health | Suitable for LAN clients | Operational weight (solo MVP) |
| --- | --- | --- | --- | --- | --- |
| **Consul** | Catalog + service DNS | Not the primary use case; DNS resolves names in the Consul domain (default `consul.`) from the catalog | Yes (registration + checks) | Yes, if clients use the Consul resolver (default port 8600) or there is a forwarder on 53 | High |
| **CoreDNS** | Pluggable DNS server | Yes (`file` with RFC 1034 `*`, or `template`) | No (without extra catalog plugins) | Yes, on port 53 (or via forwarder) | Medium-low |
| **dnsmasq** | Lightweight DNS/DHCP | Yes (`--address=/domain/ip`) | No | Yes | Low |
| **DNS in the binary** | Embedded authoritative resolver | Depends on implementation (zone with wildcard / custom logic) | No, unless the platform implements it | Yes | Variable (fewer containers; more code) |
| **mDNS / Avahi** | Zero-config discovery on `.local` | Does not cover the “unicast zone with platform-controlled wildcard” pattern | DNS-SD (another model) | Partial (OS/client support) | Low software / high fit risk |
| **Docker embedded DNS** | Names between containers on the same user-defined network | N/A for LAN | Only between containers on the network | **Does not** resolve names for phones/laptops on the LAN | — |

Detailed citations in the sections below and in [Sources](#sources).

### Table 2 — Fit to MVP constraints

| Criterion | Consul | CoreDNS | dnsmasq | Embedded DNS | mDNS | Docker DNS |
| --- | --- | --- | --- | --- | --- | --- |
| No SaaS | ✅ self-hosted | ✅ | ✅ | ✅ | ✅ | ✅ |
| One host | ✅ (server bootstrap-expect=1 documented) | ✅ | ✅ | ✅ | ✅ | ✅ |
| `name.suffix` → host IP | ⚠️ possible via artificial registration / custom domain; natural design is `*.service.consul` → instance address | ✅ | ✅ | ✅ | ❌ inadequate pattern | ❌ |
| Traefik + Docker | ✅ Catalog provider **or** Docker (independent) | ✅ (DNS only; Traefik Docker separately) | ✅ same | ✅ same | ⚠️ | Docker provider uses Docker API, not embedded DNS |
| Solo MVP weight | ❌ high | ✅ | ✅✅ | ✅ if minimal scope | ❌ weak fit | ❌ does not solve LAN |

---

## 1. HashiCorp Consul

### What it is (facts)

- Consul DNS is the primary interface for querying registered nodes/services when service mesh is disabled and the network is not Kubernetes. Source: [Consul DNS overview](https://developer.hashicorp.com/consul/docs/discover/dns).
- By default, DNS listens on `127.0.0.1:8600` and uses the `consul` domain. Consul **does not** use port 53 by default because that requires elevated privilege. Relevant parameters: `client_addr`, `ports.dns`, `domain`, `alt_domain`, `recursors`. Source: [Configure Consul DNS behavior](https://developer.hashicorp.com/consul/docs/discover/dns/configure); port defaults: [Consul ports reference](https://developer.hashicorp.com/consul/docs/reference/architecture/ports) (DNS TCP/UDP **8600**).
- `client_addr` default `127.0.0.1`; `ports.dns` default **8600**. Source: [Agent configuration — general](https://developer.hashicorp.com/consul/docs/reference/agent/configuration-file/general).
- DNS domain configurable via `domain` (default `consul.`). Source: [DNS parameters](https://developer.hashicorp.com/consul/docs/reference/agent/configuration-file/dns).

### Running as a container (one node)

Official Docker deploy documentation (single server):

```bash
docker run --name=consul-server -d \
  -p 8500:8500 -p 8600:8600/udp \
  hashicorp/consul \
  consul agent -server -ui -node=server-1 -bootstrap-expect=1 \
  -client=0.0.0.0 -data-dir=/consul/data
```

Source: [Deploy Consul server agent on Docker](https://developer.hashicorp.com/consul/docs/deploy/server/docker).

The multi-node compose on the same page also maps `8600:8600/tcp` and `8600:8600/udp` on the exposed server. Flags in the official multi-node example include `-bootstrap-expect=3` and `-retry-join=...` — **do not** invent those values beyond what the docs show.

**LAN implication:** clients would need to point DNS at `host:8600` (uncommon in DHCP/OS) **or** a forwarder on 53 (dnsmasq/CoreDNS/`iptables`) forwarding the Consul domain. HashiCorp itself documents forwarding Consul-domain queries from an existing DNS. Source: [Configure Consul DNS behavior](https://developer.hashicorp.com/consul/docs/discover/dns/configure) (“Forward DNS for Consul Service Discovery”).

### How apps register

Methods documented for VM/agent:

- Definition in the agent config directory + start/reload
- CLI `consul services register`
- HTTP `PUT /v1/agent/service/register`

Source: [Register services and health checks](https://developer.hashicorp.com/consul/docs/register/service/vm); API: [Service — Agent HTTP API](https://developer.hashicorp.com/consul/api-docs/agent/service).

The platform (daemon) would need to register/deregister on every deploy — real operational weight in the MVP.

### Fit with Traefik / Docker

- **Traefik Docker provider:** reads labels on containers; does not need Consul. Source: [Traefik & Docker](https://doc.traefik.io/traefik/v3.5/reference/install-configuration/providers/docker/).
- **Traefik Consul Catalog provider:** `providers.consulCatalog`, default endpoint `127.0.0.1:8500`, Traefik-style tags on Consul services. Source: [Traefik & Consul Catalog](https://doc.traefik.io/traefik/reference/install-configuration/providers/hashicorp/consul-catalog/).

In a single-host + Docker MVP, the Docker provider already covers `Host` routing without an external catalog.

### License (BUSL etc.)

- In August 2023 HashiCorp announced a change from MPL 2.0 to **Business Source License (BSL / BUSL) 1.1** for future product releases; APIs/SDKs/libraries generally remain MPL 2.0. Source: announcement [HashiCorp adopts Business Source License](https://www.hashicorp.com/en/blog/hashicorp-adopts-business-source-license) (summary also in official results / [Licensing FAQ](https://www.hashicorp.com/license-faq)).
- There is a **Consul Community Edition (CE)** vs Enterprise with additional features; basic discovery is in CE. Source: [Consul editions](https://developer.hashicorp.com/consul/docs/fundamentals/editions).

**Uncertainty / caveat:** legal interpretation of BSL for a commercial product that *embeds* Consul as part of a competing offering must be done with legal counsel; HashiCorp’s FAQ is the official binding guidance they publish. This document **does not** conclude compliance.

### Operational weight for a solo operator (MVP)

| Aspect | Cost |
| --- | --- |
| Extra process (server agent) | Yes |
| Non-53 DNS port | Forwarder or exotic client config |
| Register/deregister + health cycle | Code and failures to handle |
| UI/API 8500 | Surface to secure |
| BSL license | Diligence |

**Factual fit verdict:** Consul solves *distributed service discovery* well; the MVP only needs *unicast resolution of a suffix to the host IP*.

---

## 2. CoreDNS

### As a container

- Official image: `coredns/coredns` on Docker Hub; releases also as images. Source: [CoreDNS installation manual](https://coredns.io/manual/installation/), [Docker Hub coredns/coredns](https://hub.docker.com/r/coredns/coredns/).
- Official manual often uses port **1053** when not root (`-dns.port=1053`). Source: [Installation](https://coredns.io/manual/installation/).
- For LAN, the usual approach is to publish **53/udp and 53/tcp** on the host (requires privilege/`CAP_NET_BIND_SERVICE` on the process that binds — a runtime detail; the manual emphasizes the port 53 limitation).

Sketch aligned with the “mount Corefile” pattern (`-conf` flags documented in the manual; port mapping is generic Docker):

```bash
docker run --rm -d --name coredns \
  -p 53:53/udp -p 53:53/tcp \
  -v "$PWD/Corefile:/Corefile:ro" \
  coredns/coredns -conf /Corefile
```

(The manual shows `./coredns -dns.port=1053 -conf Corefile`; the Docker Hub image is the CoreDNS binary. Adjust internal/host port as needed.)

### Wildcard / rewrite to a single A

Three relevant official mechanisms:

1. **`file` plugin** — RFC 1035-style zone; classic DNS wildcards use owner `*.<domain>` (RR synthesis). Sources: [CoreDNS file](https://coredns.io/plugins/file/), [RFC 1034 §4.3.3 Wildcards](https://www.ietf.org/rfc/rfc1034.html).
2. **`template` plugin** — dynamic response by regex; official example synthesizes A from the query name. Source: [CoreDNS template](https://coredns.io/plugins/template/).
3. **`rewrite` plugin** — rewrites the *question* (and optionally the answer); useful for mapping names to another internal name, not necessarily the shortest path to “everything → same IP”. Source: [CoreDNS rewrite](https://coredns.io/plugins/rewrite/).

Illustrative example with `template` (IP and zone are from our scenario; plugin syntax is official):

```text
paas.lan:53 {
    template IN A {
        match .*\.paas\.lan\.$
        answer "{{ .Name }} 60 IN A HOST_LAN_IP"
        fallthrough
    }
    # optional: forward . <upstream> for the rest
}
```

Minimal plugins for the “dumb” case: `template` **or** `file` (+ typically `errors`/`log` in production — operational convention, not an MVP requirement).

### MVP fit

High: one container, fixed zone, zero catalog, classic port 53 for LAN DHCP.

---

## 3. dnsmasq

### Wildcard / domain → IP

Official man page:

> `--address=/<domain>/[<domain>...]/[<ipaddr>]` — Specify an IP address to return for any host in the given domains. A (or AAAA) queries in the domains are never forwarded and always replied to with the specified IP address…

Also: `/#/` matches any domain; `/etc/hosts` and DHCP leases override individual names.

Source: [dnsmasq man page](https://dnsmasq.org/docs/dnsmasq-man.html).

Example aligned with the man page:

```text
address=/paas.lan/HOST_LAN_IP
```

This makes `app.paas.lan`, `foo.paas.lan`, etc. return `HOST_LAN_IP` (behavior described for “any host in the given domains”).

### MVP fit

Very high for the minimum requirement. Less extensible than CoreDNS (plugins), but the MVP does not need that.

**Note:** there is no “official dnsmasq.org” Docker image comparable to CoreDNS’s; packaging Alpine/`dnsmasq` is common practice but **not** first-party documentation from the dnsmasq project — mark as operator implementation.

---

## 4. DNS embedded in the platform binary

### Approach (Rust / general)

In Rust, the current first-party ecosystem is **Hickory DNS** (formerly Trust-DNS):

- Crates: `hickory-server` (library for building servers), `hickory-dns` (binary). Source: [GitHub hickory-dns](https://github.com/hickory-dns/hickory-dns), [crates.io hickory-dns](https://crates.io/crates/hickory-dns).
- Manual: authoritative nameserver with zone files + `config.toml`; example run: `hickory-dns --port 2345 --config=./config.toml --zone-dir=.`. Source: [Authoritative Name server](https://hickory-dns.org/book/hickory/authoritative_nameserver.html).

General design (language-independent):

- UDP+TCP 53 thread/task (or 53 only with capability)
- In-memory or file zone with `*.paas.lan A HOST_IP`
- Control-plane updates the IP if the LAN interface changes

### Pros / cons vs sidecar container

| | Embedded in the daemon | Container (CoreDNS/dnsmasq) |
| --- | --- | --- |
| **Pros** | Fewer moving parts; IP can react to platform events without external reload | Isolation; independent upgrade; well-documented configs; DNS crash ≠ control-plane crash (or vice versa) |
| **Cons** | Bind-53 privilege on the main binary; larger security surface; reimplement DNS edge cases | One more container; sync `HOST_LAN_IP` via volume/env/reload |

**Uncertainty:** maturity of in-process `hickory-server` APIs for “dynamic wildcard without a zone file” — check docs.rs for the pinned version; the public manual emphasizes zone files + config.

---

## 5. mDNS / Avahi (limited relevance)

### What it is

Avahi implements mDNS/DNS-SD (Zeroconf / Bonjour-like) for discovery on the local network; nss-mdns allows lookup of `*.local`. Source: [avahi.org](https://avahi.org/).

### Limitations relevant to the MVP

- **RFC 6762** defines Multicast DNS; the “Wildcard Queries” section covers `qtype`/`qclass` ANY (respond with *all* matching RRs), **not** unicast owner names `*.suffix` as in RFC 1034. Source: [RFC 6762 §6.5](https://www.rfc-editor.org/rfc/rfc6762.html).
- Classic unicast wildcards (`*.example.com` synthesizing A) belong to **authoritative unicast DNS** ([RFC 1034 §4.3.3](https://www.ietf.org/rfc/rfc1034.html)), not the mDNS “each host announces its own name” model.
- Avahi exposes `AVAHI_ERR_IS_PATTERN` (−7) in the error API — evidence of special handling/rejection of pattern names in the API. Source: [Avahi error.h](http://avahi.org/doxygen/html/error_8h.html).
- Typical mDNS domain is `.local` (Bonjour/Avahi ecosystem), not an arbitrary `paas.lan` suffix configurable via DHCP the same way a unicast nameserver is.

**Factual fit conclusion:** mDNS does not replace a unicast nameserver that answers `*.paas.lan` → host IP for clients configured via DHCP. At most it can complement one-off announcements (`hostname.local`), with uneven support across devices.

---

## 6. Docker embedded DNS (what it resolves and what it does not)

Facts from Docker docs:

- Containers on **user-defined networks** use the embedded DNS at `127.0.0.11`, which resolves names/aliases **between containers on the same network** and forwards external lookups to the host’s DNS. Source: [Docker networking overview — DNS services](https://docs.docker.com/engine/network/).
- On a **user-defined bridge**, containers resolve each other by name; on the default bridge, they do not (except legacy `--link`). Source: [Bridge network driver](https://docs.docker.com/engine/network/drivers/bridge/).

**What it does NOT solve for the MVP:** notebooks and phones on the LAN are **not** members of the host’s Docker network; they do not query the daemon’s `127.0.0.11`. They need unicast DNS reachable at the host’s LAN IP (or a forwarder on the router).

---

## Minimal MVP architecture (facts + design)

```text
[LAN client] --DNS--> [dnsmasq|CoreDNS|:53] --A--> HOST_LAN_IP
[LAN client] --HTTP Host: app.paas.lan--> [Traefik|:80] --Docker provider--> [app container]
```

- DNS: domain/wildcard → `HOST_LAN_IP` (dnsmasq `address=` or CoreDNS `file`/`template`).
- Proxy: Traefik labels `traefik.http.routers.*.rule=Host(\`app.paas.lan\`)` ([Docker provider doc](https://doc.traefik.io/traefik/v3.5/reference/install-configuration/providers/docker/)).
- LAN operation: DHCP option 6 (or manual config) pointing at the host IP — a home/lab networking detail; not a PaaS feature itself.

### Compose sketch (backed ideas only)

**Consul (official):** see `docker run` / compose in [Deploy Consul server on Docker](https://developer.hashicorp.com/consul/docs/deploy/server/docker).

**CoreDNS:** image `coredns/coredns`, `-conf /Corefile`, publish 53 — see [installation](https://coredns.io/manual/installation/) + Hub.

**dnsmasq:** `address=/paas.lan/HOST_LAN_IP` from the [man page](https://dnsmasq.org/docs/dnsmasq-man.html); container packaging is the operator’s responsibility.

**Traefik:** `providers.docker: {}` and label `Host(\`...\`)` — [doc](https://doc.traefik.io/traefik/v3.5/reference/install-configuration/providers/docker/).

Do not invent undocumented flags.

---

## Phase 2 — Consul-style discovery (without rewriting the hostname model)

| Step | Change | Impact on the model |
| --- | --- | --- |
| Keep wildcard DNS → host | None | Same LAN clients |
| Traefik stays on Docker provider | None | Same apps |
| Optional: start Consul CE, register services, migrate Traefik to `consulCatalog` | Config provider | HTTP hostname can stay |
| Optional: Consul health checks | Observability / drain | Internal |

It would only be a large rewrite if DNS started pointing at container IPs or if mesh/upstreams became the primary path ([Consul DNS vs upstreams](https://developer.hashicorp.com/consul/docs/discover/dns)).

---

## Recommendation (opinion)

**MVP:** implement **“dumb” DNS** with **dnsmasq** (smaller surface) **or CoreDNS** (if plugins/`template`/selective forward are already anticipated). Practical preference: **dnsmasq** if the only requirement is `address=/paas.lan/HOST_IP`; **CoreDNS** if the platform wants a versioned Corefile and future extensions without changing DNS servers.

Couple **Traefik Docker provider** for `Host` routing. Do not introduce Consul in the MVP.

**Phase 2:** consider Consul (CE, self-hosted) only if a real need arises for multi-service catalog, health-driven routing, or multiple nodes — not for generic “service discovery.” LAN wildcard DNS can remain.

**Consul in the MVP:** **overkill** — cost (agent, registration, port 8600/forwarder, BSL diligence) with no benefit for the done criterion (`*.suffix` → host + HTTP Host routing).

**DNS embedded in the binary:** defer until the MVP proves the flow with a sidecar; only worth it if the cost of a DNS container is unacceptable.

**mDNS:** not as the primary MVP solution.

---

## Sources

1. HashiCorp — [Consul DNS overview](https://developer.hashicorp.com/consul/docs/discover/dns)  
2. HashiCorp — [Configure Consul DNS behavior](https://developer.hashicorp.com/consul/docs/discover/dns/configure)  
3. HashiCorp — [Deploy Consul server agent on Docker](https://developer.hashicorp.com/consul/docs/deploy/server/docker)  
4. HashiCorp — [Consul ports reference](https://developer.hashicorp.com/consul/docs/reference/architecture/ports)  
5. HashiCorp — [Agent configuration (general / ports / client_addr)](https://developer.hashicorp.com/consul/docs/reference/agent/configuration-file/general)  
6. HashiCorp — [DNS parameters (`domain`, `recursors`)](https://developer.hashicorp.com/consul/docs/reference/agent/configuration-file/dns)  
7. HashiCorp — [Register services and health checks](https://developer.hashicorp.com/consul/docs/register/service/vm)  
8. HashiCorp — [Service — Agent HTTP API](https://developer.hashicorp.com/consul/api-docs/agent/service)  
9. HashiCorp — [Consul editions](https://developer.hashicorp.com/consul/docs/fundamentals/editions)  
10. HashiCorp — [Adopts Business Source License](https://www.hashicorp.com/en/blog/hashicorp-adopts-business-source-license), [Licensing FAQ](https://www.hashicorp.com/license-faq)  
11. CoreDNS — [Installation](https://coredns.io/manual/installation/)  
12. CoreDNS — [file plugin](https://coredns.io/plugins/file/)  
13. CoreDNS — [template plugin](https://coredns.io/plugins/template/)  
14. CoreDNS — [rewrite plugin](https://coredns.io/plugins/rewrite/)  
15. Docker Hub — [coredns/coredns](https://hub.docker.com/r/coredns/coredns/)  
16. dnsmasq — [Man page](https://dnsmasq.org/docs/dnsmasq-man.html)  
17. Traefik — [Docker provider](https://doc.traefik.io/traefik/v3.5/reference/install-configuration/providers/docker/)  
18. Traefik — [Consul Catalog provider](https://doc.traefik.io/traefik/reference/install-configuration/providers/hashicorp/consul-catalog/)  
19. Docker Docs — [Networking overview (embedded DNS)](https://docs.docker.com/engine/network/)  
20. Docker Docs — [Bridge driver / user-defined networks](https://docs.docker.com/engine/network/drivers/bridge/)  
21. IETF — [RFC 1034 Domain Concepts (§4.3.3 Wildcards)](https://www.ietf.org/rfc/rfc1034.html)  
22. IETF — [RFC 6762 Multicast DNS](https://www.rfc-editor.org/rfc/rfc6762.html)  
23. Avahi — [Project site](https://avahi.org/), [error.h (`AVAHI_ERR_IS_PATTERN`)](http://avahi.org/doxygen/html/error_8h.html)  
24. Hickory DNS — [GitHub](https://github.com/hickory-dns/hickory-dns), [Authoritative nameserver manual](https://hickory-dns.org/book/hickory/authoritative_nameserver.html), [crates.io](https://crates.io/crates/hickory-dns)

---

## Remaining uncertainties

- Exact behavior of `address=/paas.lan/IP` vs need for `address=/.paas.lan/IP` on specific dnsmasq versions: validate with `dig` on the pinned version (man describes “any host in the given domains”; some tutorials use `/.domain/` — confirm against the man page of the build in use).
- Legal interpretation of BSL for embedded redistribution in a commercial product.
- mDNS/`*.local` support on modern mobile clients — varies by OS; not researched exhaustively here.
- In-process `hickory-server` APIs for dynamic zones without file reload — check on the chosen version.
