# MVP scope

Historical scope from the initial implementation. The current product direction
and next delivery are defined in [Product vision](vision.md) and the
[macOS Compose MVP plan](mvp-plan.md). This document is retained as historical
context; its exclusions of a web console and Compose are no longer current.
Its HTTP-only TLS decision is also historical. Bootstrap now creates a private
CA and serves Application Hostnames over HTTPS. Each Consumer must trust the
public CA certificate once.

Grilling session decisions (scope confirmed).  
Last updated: 2026-08-04.

## MVP goal

On a LAN Host, the Operator downloads the Platform binary, starts it (Bootstrap), and can Deploy Applications (image pull **or** local build). Consumers reach them over HTTP via Application Hostnames resolved by DNS from the Platform’s own stack — **without depending on an external SaaS** for the happy path.

> Clarification: “no internet / no external service” means no third-party SaaS for DNS/control. It does **not** mean a full air gap (e.g. `docker pull` from a public registry may still happen when the network is available).

## Closed decisions

| # | Topic | Decision |
|---|---|---|
| 1 | Primary environment | **LAN** Host (not VPS-first) |
| 2 | Roles | Single Operator; **Consumers** on the LAN use the apps |
| 3 | Control plane | **Daemon + HTTP JSON API + CLI** (no management UI in MVP; same API can serve a future web console) |
| 4 | Application runtime | **Docker container** (not a bare host process) |
| 5 | Platform | **One binary** that starts Infra and Application containers as needed |
| 6 | Deploy in MVP | **Image pull** **or** **local build** (Dockerfile/context). **No GitHub/Git in MVP** |
| 7 | Local DNS | **Required** for MVP “done” |
| 8 | TLS | **HTTP only** in MVP (HTTPS/local CA later) — for Consumer traffic; Operator API is also HTTP on the LAN in MVP |
| 9 | Hostname | Default `name.<DNS Suffix>`; explicit **override** allowed |
| 10 | DNS / discovery | **dnsmasq** (Infra container) — dumb DNS `*.<suffix>` → Host IP. Consul rejected for MVP; DNS-in-binary considered and not chosen |
| 11 | Platform state | **Files the Platform owns**, under `~/.config/self-host/state/` ([ADR-0018](adr/0018-platform-state-in-files.md)). PostgreSQL 18 was the original choice and made Docker a dependency of reading configuration |
| 12 | HTTP proxy | **The Platform's own process**, routing by `Host` ([ADR-0019](adr/0019-embedded-http-proxy.md)). Traefik in a container was the original choice and made public HTTPS depend on Docker |
| 13 | Logs | **`self-host logs <app>`** — stream over **HTTP** from Docker; no Platform-owned retention in MVP |
| 14 | CLI | Disco-style spaced subcommands (`apps add`, not `apps:add`). Binary: **`self-host`**. Resource: **`apps`**. Quickstart: `init` → `apps add` (`--image` \| `--path`) → `list` / `logs` / `remove`. No GitHub/`git push`/`init user@host` in MVP |
| 15 | Suffix on `init` | **`self-host init --dns <suffix>`**. Default: **`home.lan`** (avoid `.local` / mDNS). Multiple suffixes = post-MVP |
| 16 | Consumer DNS | After `init`, the CLI **prints instructions** (Host IP + point resolvers). No automatic router/DHCP integration |
| 17 | Env vars | **`self-host apps env set|get|unset`** (manage after create). `--env-file` out of MVP |
| 18 | Language | **Rust** (CLI + daemon binary; HTTP API via axum or equivalent; Docker via Engine API) |
| 19 | Ports / network | LAN: **:80** and **:443** (Consumers) + **:53** (DNS), all bound by the Platform; an Application's web target is published on loopback only. **Operator HTTP API also on the LAN** (CLI / console on another device). Arbitrary app port publish out of MVP |
| 20 | API auth | **API key** generated on `init`; CLI stores local config. No mTLS in MVP. gRPC rejected in favor of HTTP JSON (ADR-0007) |

## Explicitly out of MVP

- Web management UI
- GitHub / git clone / webhooks / GHCR as a required source (candidate for v0.2)
- Kubernetes / controller / Operator
- Let’s Encrypt / HTTPS
- Multi-operator / accounts
- Multi-host / cluster
- Feature parity with Dokploy/Coolify/Kubero
- Multiple DNS Suffixes on one Platform (MVP = one, chosen at `init`)
- `init user@host` over SSH (CLI on laptop → remote Host)
- GitHub / git push to deploy

## DNS / discovery

**Closed.** See decisions #10 and #12 and ADR `0003`.

Supporting research: [docs/research/lan-dns-service-discovery.md](./research/lan-dns-service-discovery.md).

## Done criteria (draft)

1. Bootstrap: `self-host init [--dns]` configures the Host, generates an API key, prints DNS instructions
2. CLI (on the Host or another LAN device) authenticates with the API key over **HTTP**
3. `apps add` works for both image and local build
4. A Consumer on the LAN resolves the Hostname and gets HTTP from the Application
5. `self-host logs <app>` and `apps env set|get|unset` work
6. No external SaaS account on the happy path

## Next

- `/to-tickets` against epic [#1](https://github.com/momoi-labs/self-host/issues/1) — test seam = Platform Operator HTTP API
