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

**Virtual machine**:
A persistent workspace owned by the Operator, with selected development tools,
repositories and independent access. Its lifecycle is separate from published
Applications.
_Avoid_: development environment (previous UI name), development image (a build artifact), Application (a published workload)

**Deploy**:
The Operator action that makes an Application available on the LAN from its definition and configuration.
_Avoid_: release, publish, ship (as official synonyms)

**Application Hostname**:
The DNS name Consumers use to reach an Application on the LAN, usually `name.<suffix>` with an optional explicit override. Served by a Record the Platform manages.
_Avoid_: URL (URLs include scheme/path), public domain

**Hostname Alias**:
An additional Application Hostname the same Application answers on, alongside its Hostname. Aliases are always explicit: changing a Hostname never leaves one behind on its own.
_Avoid_: CNAME, redirect (an alias serves the Application, it does not forward)

**DNS Suffix**:
The configurable local zone suffix for the Platform under which Application Hostnames are derived. The MVP has a single DNS Suffix, set via `self-host init --dns` (default **`home.lan`**, intentionally not `.local` because of mDNS conflicts). Multiple suffixes are out of MVP scope.
_Avoid_: domain, TLD, home.local (as default)

**Zone**:
Everything the Platform answers for under the DNS Suffix: the wildcard, the Records it manages for itself, for Applications and for Virtual machines, and the Records the Operator typed. One Zone per Platform, served by the Platform's own DNS (ADR-0017, ADR-0025).
_Avoid_: domain, DNS config, the resolver (that is what asks the Zone)

**Record**:
One answer in the Zone: a name under the DNS Suffix, a Record Type, a value, a TTL, an optional description and an owner. A Record with an explicit name wins over the wildcard. Owners are the Platform (the wildcard, `admin`), an Application (its Hostname and Aliases), a Virtual machine (its name), or the Operator, who creates, edits and deletes their own.
_Avoid_: entry, mapping, DNS rule

**Record Type**:
The DNS type of a Record. `A` is the only type the Operator can create today; the Zone is keyed by name and type so others slot in later.
_Avoid_: address family, IP version (as the type of a Record)

**TTL**:
How long a resolver may cache a Record. Fixed at 60 seconds for every Record the Platform serves, so an address change reaches Consumers within a minute.
_Avoid_: expiry, cache time

**Compose Application**:
An Application defined by a Docker Compose file the Operator supplies. The Platform runs a rendered project named after the Application, one container per service, and routes the Hostname to the Application's Web Target.
_Avoid_: stack, docker-compose app, project (as the user-facing term; the project is what the Platform renders)

**Web Target**:
Where an Application's Hostname routes to: the Compose service and container port of a Compose Application, or the loopback port a Native Application's process listens on. Resolved when the definition is accepted and recorded on the Application. An unpublished Application has none.
_Avoid_: backend, main service, entrypoint

**Runtime**:
How an Application runs on the Host: in a container (Docker, one container or a Compose project) or as a native process tree under an Application Account. Recorded on the Application; every Application that predates the term runs in a container.
_Avoid_: engine, executor, backend (as the name for how an Application runs)

**Native Application**:
An Application whose Runtime is a process tree on the Host, run under its own Application Account, with no container. The Platform records the shape today and runs it in a later slice.
_Avoid_: bare-metal app, host process (as the product term), s6 service (the supervisor is a detail)

**Application Account**:
The dedicated Host account a Native Application runs as: an execution identity the Platform owns, never root and never the Operator's own login.
_Avoid_: service user, system user, login (an Application Account is not for people)

**Publication**:
Whether Consumers reach an Application by its Hostname. A published Application has a Hostname, a Record, a route and a Web Target; an unpublished one, such as a worker or a database, has none of them. Chosen when the Application is created. Distinct from Deploy, which makes the Application run.
_Avoid_: exposure, visibility, public/private (the LAN is the only audience)

**Application Variable**:
A named value the Operator sets on an Application, which the Application's services read as an input. How it reaches a Compose service, copied into every service or only where the file references it, is recorded on the Application.
_Avoid_: env var (as the product term), secret (a Variable may hold one, but is not one by definition), config value

**Platform Infra**:
Components the Platform once started for itself, as containers, rather than Operator Applications. There are none left: the Platform keeps its state in files it owns (ADR-0018) and serves DNS (ADR-0017) and HTTP (ADR-0019) from its own process. Docker runs Applications.
_Avoid_: dependencies (ambiguous with app deps), sidecars

**Task**:
One console action the Platform carries out on its own worker after accepting the request: create, delete, stop, start, restart or configure an Application, a Virtual machine or a custom image. A Task is `pending`, then `running`, then `completed` or `failed`; the audit event with the same id is its record. Tasks on one object run one at a time, oldest first (ADR-0027).
_Avoid_: job, background operation, cron (a Task is not scheduled for a time)

**Platform State**:
Everything the Operator configured, as the Platform records it: settings, credentials, the Zone's Records, Virtual machines, custom images, the audit history, and each Application's identity, publication, definition and environment. Authoritative, owned by the daemon, and separate from anything generated from it. Kept in one SQLite file the daemon opens in-process (ADR-0027); the Compose definitions stay verbatim on disk beside it.
_Avoid_: database (as the name of a component that runs), cache, the state store (as a component that runs)
