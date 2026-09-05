# Product vision

Self-host helps developers and homelab enthusiasts prepare a personal Host and
operate Applications on their LAN. It brings Bootstrap and Application lifecycle
management into one workflow, using existing tools instead of requiring the
Operator to assemble and maintain every integration manually.

This is the approved product direction. It describes the next MVP, not the
capabilities already implemented. The [implementation plan](mvp-plan.md) defines
the first delivery and its acceptance criteria.

## Audience and value

One Operator manages the homelab; Consumers use its Applications. Examples
include AI assistants, Git, web applications, databases and automations.
The Platform operates locally without requiring a commercial deployment service.
Applications may still depend on external providers, such as a model API.

The first Host is an Apple Silicon MacBook used as a personal server. The
Operator's Linux workstation remains a development machine and accesses
Applications through the browser. A second household Consumer also uses the
browser. This does not introduce multiple Platform Operators.

## Responsibilities

- Bootstrap the Platform, its required runtime and Platform Infra.
- Accept Application definitions and the configuration needed to run them.
- Install, start, stop and restart Applications; expose their state and logs.
- Make Applications reachable on the LAN and preserve their persistent data.
- Restore the Platform and Applications configured to start after Host restart,
  without requiring an interactive login.

Self-host does not own the assistant's memory model, planning workflow, coding
harness or messaging integrations. Hermes owns its conversation behavior and
access controls. T3 Code is a possible development tool, not part of the Platform.

## First useful slice

Starting with a Mac that meets documented prerequisites, the Operator installs
self-host, provides a Hermes Compose definition through the web console,
completes the required configuration and opens a working chat from another
machine on the LAN. Persistent data survives container recreation. After the
Mac restarts, the Platform and Hermes return without a user logging in.

Hermes is the first Application used to validate the workflow. The implementation
must support Compose Applications rather than encode a Hermes-only deployment
path. Its official example is a reference, not a guarantee that startup alone
completes provider authentication or browser configuration.

## Decisions

| Area | Decision |
| --- | --- |
| Initial Host | macOS, validated on the available Apple Silicon Mac |
| Bootstrap | Shell installer prepares self-host, Docker runtime and Compose |
| Platform supervision | launchd LaunchDaemon, independent of a login session |
| MVP Application definition | Docker Compose supplied through the web console |
| First Application | Hermes, with browser chat for two household Consumers |
| Persistence | Docker-mounted persistent storage for configuration and data |
| Native Applications | s6 after the MVP; no host-level s6 installation now |

See [ADR-0013](adr/0013-macos-bootstrap-with-launchd.md) and
[ADR-0014](adr/0014-compose-applications-and-future-native-supervision.md).
No universal manifest spanning Compose and native processes is selected.
Homebrew and mise are possible installation tools, not mandatory dependencies
of the approved slice.

## Boundaries of the MVP

The MVP covers one Host and the browser workflow. Native Applications, other Host
operating systems, T3 Code deployment, messaging channels, a catalog, backups and
automatic updates are deferred. WhatsApp and Signal are possible later channels.
Neither 1Password integration nor FileVault configuration is a product feature
required by this slice.

Host prerequisites still have to permit unattended startup. Selecting launchd
does not prove that the Docker runtime, storage and credentials are available
before login. FileVault unlock, sleep and physical power recovery are separate
Host conditions to document and validate where they affect the promised behavior.

## What can be reused

The repository already has a Rust CLI and daemon, HTTP JSON API, web console,
container deployment, logs, DNS, TLS and hostname aliases. Application identity,
state storage and routing provide useful starting points.

The existing Application path assumes one container and an HTTP backend on port
80. It lacks the Compose Application workflow and volume configuration needed
here. Docker must already be prepared, and the Platform daemon has no boot
installation. The initial review also found loopback Host IP selection and
incomplete environment-variable and additional API-key paths. These are code
review findings, not results from a Mac installation test; fix what blocks this
slice without turning it into a general cleanup project.

## Delivery approach

Deliver Bootstrap and Application operation together in a small usable cycle.
Validate the Docker runtime's unattended startup first, then use the Hermes
workflow to test the product on the real Host. Choose the next increment from
actual use rather than building a complete homelab platform in advance.

Supporting research covers [Hermes](research/hermes-installation.md),
[supervision](research/self-host-supervision.md) and optional
[1Password integration](research/onepassword-secrets.md). Deferred native
Application work is tracked in [TODO.md](../TODO.md).
