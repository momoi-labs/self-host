# Self-host PaaS (LAN)

A self-hosted platform for publishing applications on a local network, run by one operator, used by others on the LAN — without depending on an external service (SaaS) for the happy path.

## Language

**Platform**:
The control system installed on the host: the binary/daemon that bootstraps infra and manages application lifecycle.
_Avoid_: PaaS (as a vague synonym), cluster, Kubernetes

**Application**:
A workload managed as a unit by the Platform and made available to Consumers on the LAN. An Application may consist of several services; its identity is independent of how those services run.
_Avoid_: binary (reserved for the Platform), service (overloaded), site

**Bootstrap**:
Preparing the Host to run the Platform and bringing up the Platform Infra required to operate.
_Avoid_: install script as a domain concept, setup

**Operator**:
The person who configures the Platform and deploys and manages Applications. The MVP has a single Operator.
_Avoid_: admin, user, developer (as product roles)

**Consumer**:
Someone who accesses already-published Applications on the LAN (HTTP). They do not operate the Platform.
_Avoid_: end user, client, visitor

**Host**:
The LAN machine where the Platform, Platform Infra and Applications run.
_Avoid_: node, server, VPS (VPS is not the primary MVP environment)

**Deploy**:
The Operator action that makes an Application available on the LAN from its definition and configuration.
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
