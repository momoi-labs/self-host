# Self-host PaaS (LAN)

A self-hosted platform for publishing applications on a local network, run by one operator, used by others on the LAN — without depending on an external service (SaaS) for the happy path.

## Language

**Platform**:
The control system installed on the host: the binary/daemon that bootstraps infra and manages application lifecycle.
_Avoid_: PaaS (as a vague synonym), cluster, Kubernetes

**Application**:
A workload published for consumers on the LAN, run as a Docker container (from a ready image or a local build).
_Avoid_: binary (reserved for the Platform), service (overloaded), site

**Bootstrap**:
Starting the Platform on the host from the binary (e.g. download + run), bringing up the infra required to operate.
_Avoid_: install script as a domain concept, setup

**Operator**:
The person who publishes and manages Applications via the CLI (and the Platform HTTP API). The MVP has a single Operator.
_Avoid_: admin, user, developer (as product roles)

**Consumer**:
Someone who accesses already-published Applications on the LAN (HTTP). They do not operate the Platform.
_Avoid_: end user, client, visitor

**Host**:
The LAN machine where the Platform runs and where Application (and infra) containers start.
_Avoid_: node, server, VPS (VPS is not the primary MVP environment)

**Deploy**:
The Operator action that makes an Application available on the LAN from an existing Docker image or a local build (Dockerfile/context).
_Avoid_: release, publish, ship (as official synonyms)

**Application Hostname**:
The DNS name Consumers use to reach an Application on the LAN, usually `name.<suffix>` with an optional explicit override.
_Avoid_: URL (URLs include scheme/path), public domain

**Hostname Alias**:
An additional Application Hostname the same Application answers on, alongside its Hostname. Aliases are always explicit: changing a Hostname never leaves one behind on its own.
_Avoid_: CNAME, redirect (an alias serves the Application, it does not forward)

**DNS Suffix**:
The configurable local zone suffix for the Platform under which Application Hostnames are derived. The MVP has a single DNS Suffix, set via `self-host init --dns` (default **`home.lan`**, intentionally not `.local` because of mDNS conflicts). Multiple suffixes are out of MVP scope.
_Avoid_: domain, TLD, zone (as raw DNS jargon in the glossary), home.local (as default)

**Platform Infra**:
Components the Platform starts for itself (e.g. HTTP traffic proxy, local DNS, state store) — not Operator Applications.
_Avoid_: dependencies (ambiguous with app deps), sidecars
