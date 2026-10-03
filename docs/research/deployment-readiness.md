# Deployment readiness assessment

This is the baseline assessment from before the implementation waves. See
[Wave 4 validation](wave-4-validation.md) for delivered behavior, test evidence
and the remaining release gates.

The original requirements called for public certificate management, path-based
routing, a complete source deployment workflow, guided local PostgreSQL creation and native
Application execution. The container runtime and embedded proxy provided
part of that foundation.

This document contains reusable product requirements and source-code findings.
It omits deployment identities, inventory, provider choices, access details,
configuration snapshots and measurements. Examples are synthetic. Recheck the
source findings against the release selected for implementation.

## Required capabilities

| Capability | Required outcome |
| --- | --- |
| Public HTTPS | Issue and renew trusted certificates for unrelated hostnames and aliases. Select the correct certificate for each request and preserve certificate state across restart. |
| Web routing rules | Map a hostname and path to a local port. Support native and container targets, explicit prefix handling and deterministic precedence. |
| Repeatable deployment | Reproduce source checkout, build inputs and deployment triggers, or explicitly replace that workflow with image publishing and the deployment API. |
| Compose configuration | Accept or normalize supported definitions, resolve Application variables consistently and preserve commands, dependencies and storage semantics. |
| Local PostgreSQL | Offer a guided creation flow with credentials, durable storage, health reporting and private Application connectivity. Support importing data and verified backup/restore. |
| Native Applications | Run processes through s6 under dedicated non-root accounts, with lifecycle controls, logs, persistence and recovery after Host restart. |
| Linux resource isolation | Manage Application accounts and cgroup contexts. Limit the entire process tree and define filesystem access separately. |
| Recovery | Preserve Application data, queued work and Platform State. Rehearse restore and recovery from a failed deployment before moving traffic. |

The initial scope requires one Platform Operator. Linux accounts used to run
Applications are separate from console logins. This does not require a general
identity-management product or support for additional database engines.

## Baseline source capability matrix

These findings describe the implementation inspected before the waves, not the
current working tree or a deployed system.

| Capability | Status | Current behavior and consequence | Source |
| --- | --- | --- | --- |
| Linux installation | Existing | Release packaging supports Linux. Docker and Compose remain Host prerequisites for container Applications. | [Packaging](../../.goreleaser.yml), [runtime](../../src/docker.rs) |
| Host supervision | Partial | Linux installation does not provide the complete daemon supervision workflow. Service identity, permissions and boot recovery need deployment configuration. | [Installer](../../install.sh), [operating guide](../operating.md) |
| Container images | Existing | Applications can run from registry images or local Dockerfile builds. Build paths refer to the Host filesystem. | [Application lifecycle](../../src/apps.rs), [runtime](../../src/docker.rs) |
| Registry credentials | Partial | Docker can use the service account's credential configuration. The Platform has no dedicated registry credential management flow. | [Runtime](../../src/docker.rs), [API](../../src/lib.rs) |
| Git source deployment | Absent | No integrated clone/update, branch tracking or source webhook workflow exists. External image publishing is an alternative that still needs implementation and verification. | [Deployment inputs](../../src/lib.rs), [Application lifecycle](../../src/apps.rs) |
| Custom image creation | Partial | Supplied Dockerfiles use a generated build context. This does not replace repository checkout and source deployment. | [Custom images](../../src/custom_images.rs) |
| Multiple Compose services | Existing | Web processes, workers and databases can run together. Supported commands, entrypoints, users and working directories pass through. | [Compose implementation](../../src/compose_app.rs) |
| Compose compatibility | Partial | The parser accepts an allowlist. Build directives, environment files, custom networks, secrets, configs, extra groups and other unsupported keys are rejected. | [Parser](../../src/compose_app.rs) |
| YAML extensions and merges | Partial | Extension keys are rejected and merge keys are not expanded before validation. Definitions need normalization or parser support. | [Parser](../../src/compose_app.rs) |
| Environment variables | Partial | Saved Application variables are injected into each service. They do not provide the Compose CLI's interpolation environment and can spread credentials to services that did not reference them. | [Renderer](../../src/compose_app.rs), [Compose invocation](../../src/docker.rs) |
| Existing named volumes | Partial | Platform-generated project names produce new volume names. External volumes are rejected and declared volume options are discarded. Data adoption needs an explicit mapping or a verified copy. | [Volume handling](../../src/compose_app.rs) |
| Bind mounts | Partial | Short-syntax absolute mounts work. Relative paths are rewritten under Application data; long syntax is rejected. Files must not be mistaken for directories. | [Mount handling](../../src/compose_app.rs), [project writer](../../src/docker.rs) |
| PostgreSQL provisioning | Partial | PostgreSQL can run inside a Compose Application. There is no guided database creation or app-connection flow. A separately managed database also needs a lifecycle without an HTTP Web Target. | [Creation form](../../console/src/components/AppForm.tsx), [deployment](../../src/apps.rs), [Web Target](../../src/compose_app.rs) |
| Dependencies and readiness | Partial | Dependency and healthcheck declarations pass through. Process state is not a readiness guarantee, and deployment does not wait for overall application health. | [Compose runtime](../../src/docker.rs), [status](../../src/apps.rs) |
| One-shot services | Partial | Completion dependencies pass through, but an intentionally exited service can make the Application appear failed. This needs explicit handling if adopted. | [Status and recovery](../../src/apps.rs), [restart defaults](../../src/compose_app.rs) |
| Restart policies | Existing | Explicit container policies are preserved; the renderer supplies a default when absent. | [Renderer](../../src/compose_app.rs) |
| Container resource limits | Partial | Supported deployment resource declarations pass through. Enforcement needs verification against the rendered definition and runtime. | [Compose implementation](../../src/compose_app.rs) |
| Replicas and orchestration | Limited | Fixed container names prevent ordinary Compose scaling. The Platform does not provide a Swarm orchestration workflow. | [Container naming](../../src/compose_app.rs), [runtime](../../src/docker.rs) |
| Network isolation | Partial | Compose services join a project network and a shared Application network. Cross-Application reachability and reused service aliases need review. | [Network rendering](../../src/compose_app.rs) |
| Workload adoption | Absent | Existing unmanaged containers or Host processes have no supported import/adoption flow. | [Ownership and lifecycle](../../src/apps.rs), [runtime](../../src/docker.rs) |
| Hostnames and aliases | Existing with limits | Explicit hostnames and aliases route to one Web Target. DNS ownership and certificate coverage are separate requirements. | [Routing validation](../../src/apps.rs), [publication](../../src/routes.rs) |
| Path routing | Absent | Routes are keyed by hostname. Shared-host path targets, precedence and prefix stripping are not represented. | [Proxy](../../src/proxy.rs), [Application state](../../src/store.rs) |
| Multiple route targets | Absent | One service and port are recorded per Application. Container proxy labels do not configure the embedded proxy. | [Application state](../../src/store.rs), [publication](../../src/routes.rs) |
| Public certificate lifecycle | Absent | The current local-CA workflow does not provide public issuance, renewal or per-hostname certificate management. | [TLS](../../src/tls.rs), [proxy](../../src/proxy.rs) |
| Manual certificates | Partial | The proxy loads a certificate and key at startup. There is no complete import, renewal and live-reload workflow. | [TLS loading](../../src/proxy.rs), [validation](../../src/tls.rs) |
| HTTP and WebSockets | Existing | HTTP redirects, streaming and WebSocket upgrades are implemented. Other transports require separate validation or implementation. | [Proxy](../../src/proxy.rs) |
| Trusted upstream proxies | Partial | Forwarded identity is rebuilt from the immediate peer. Preserving original client identity behind an upstream proxy requires an explicit trust policy. | [Header handling](../../src/proxy.rs) |
| Operator authentication | Existing | Bearer keys protect API access. Additional keys are revocable but have full Operator privileges. | [API authentication](../../src/lib.rs), [terminal authentication](../../src/terminal.rs) |
| Control-plane exposure | Partial | The plaintext API needs a restricted listener. The HTTPS console hostname follows the configured suffix rather than an independently selected console hostname. | [Server configuration](../../src/main.rs) |
| External DNS | Supported operating choice | The daemon can run without serving DNS. Public resolver exposure is unnecessary for externally managed names. | [Server startup](../../src/main.rs), [DNS](../../src/dns.rs) |
| IPv6 | Absent in publication path | Direct IPv6 service requires implementation. An upstream edge serving IPv6 does not itself require an IPv6 origin. | [Host addresses](../../src/host_addresses.rs), [listeners](../../src/main.rs) |
| Image updates | Implemented in inspected checkout | Registry image refresh exists in the inspected source. Verify inclusion in the selected release and distinguish registry images from locally built images. | [Lifecycle](../../src/apps.rs), [settings](../../src/settings.rs) |
| Deployment rollback | Absent | No health-gated traffic switch, revision history or automatic rollback workflow exists. A previous pinned image can be redeployed manually. | [Lifecycle](../../src/apps.rs) |
| Tasks and audit | Existing | Lifecycle work is asynchronous and serialized per object, with audit/state records and interrupted-work recovery. | [Tasks](../../src/tasks.rs), [audit](../../src/audit.rs) |
| Logs and terminals | Existing for containers | Container logs and terminals are available. Native Applications need equivalent integration. Host log retention remains an operating concern. | [Runtime](../../src/docker.rs), [terminal](../../src/terminal.rs) |
| Metrics | Partial | Container and proxy metrics have in-memory history. Durable monitoring, alert delivery and native process metrics need separate work. | [Metrics](../../src/metrics.rs) |
| Backup and restore | Partial | Manual Platform State backup is documented. Scheduled off-host backups and Application data restore are not complete product workflows. | [Operating guide](../operating.md), [state](../../src/file_store.rs) |
| Native Application lifecycle | Absent | Status, logs, metrics and terminals assume Docker. Loopback proxy transport exists, but native process ownership and supervision do not. | [Lifecycle](../../src/apps.rs), [routes](../../src/routes.rs), [native direction](../adr/0014-compose-applications-and-future-native-supervision.md) |
| Linux accounts and cgroups | Absent for native apps | No per-Application Host account provisioning, privilege-drop policy or cgroup lifecycle exists. | [Application state](../../src/store.rs), [lifecycle](../../src/apps.rs) |

## Native execution without root

Use s6 for native Application supervision, consistent with
[ADR-0014](../adr/0014-compose-applications-and-future-native-supervision.md).
Provide start, stop, restart, status, logs and recovery without an interactive
login. Application data must outlive process restarts.

Each native Application needs a dedicated account and owned data directories.
Its main process, workers, terminals and build commands must run under that
account. Administrative provisioning may create accounts and resource contexts;
it must drop privileges before executing any Application-supplied command.

Reject an empty account, an unknown account and UID 0. If identity setup fails,
do not start the Application or retry as root. Set its own home and environment,
remove inherited administrative credentials and capabilities, and prevent
privilege escalation. Do not grant sudo or container-engine access by default.

The s6 identity-switching tool changes UID, GID and supplementary groups before
executing a program. Its empty-account behavior skips that change, so callers
must validate the account. Keep supervisor wrappers outside directories the
Application can modify. Apply privilege dropping to lifecycle hooks as well.
[s6 identity switching](https://skarnet.org/software/s6/s6-setuidgid.html),
[s6 supervision](https://skarnet.org/software/s6/s6-supervise.html).

Assign the process to its cgroup before it starts spawning children. CPU,
memory and task limits must apply to the entire tree. Stop and restart must
clean up descendants, including detached processes. Respect the Host service
manager's ownership and delegation of the cgroup hierarchy.

Cgroups control resources; filesystem and network isolation require separate
restrictions. Validate that the Application can access its assigned data but
cannot access another Application's private files or Platform credentials.
[Linux cgroup documentation](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html).

## Web routing behavior

Use the existing embedded proxy. A rule needs a hostname, path match, local
target and explicit prefix handling. A more-specific rule takes precedence
over the root rule. Configuration changes must not restart unrelated apps.

The following examples contain no deployment values:

| Synthetic hostname | Path | Synthetic target | Behavior |
| --- | --- | --- | --- |
| `native.example.invalid` | `/` | `<loopback>:<native-port>` | Preserve the path, including API and WebSocket requests. |
| `web.example.invalid` | `/example` | `<loopback>:<web-port>` | Preserve or strip the prefix according to the upstream app. |

Test assets, redirects, query strings, cookies and WebSockets. Prefix stripping
alone does not make every app work under a subpath. Applications sharing a
hostname share a browser origin; use separate hostnames when they must not
share that authority. Backend ports should remain private, and authentication
must hold through the final proxy path.

## Local PostgreSQL behavior

Provide a creation flow for a database, credentials and persistent storage.
Expose connection details to the consuming Application through a private path.
Report database readiness and preserve data across restart and redeploy.

Reuse the container runtime where appropriate. Whether the flow creates an
app-owned service or a separately managed instance remains an implementation
decision. A separate instance needs a lifecycle without an HTTP Web Target.
Do not combine data migration with a major-version upgrade by default.

Prove import and restore with isolated data. Preserve ownership and the data
directory expected by the chosen image. Do not attach one live database
directory to two writers. Database backup and queue persistence are separate
from a backup of Platform configuration.

## Migration controls

- Select a released build that contains the required behavior.
- Reproduce build inputs and deployment triggers, including any replacement
  image-publishing workflow.
- Map storage before changing runtime ownership. Keep private migration inputs
  outside repository documents.
- Preserve or deliberately drain queued work before replacing its runtime.
- Restrict management listeners and keep database ports private.
- Supervise the Platform and native Applications across Host restart.
- Verify backups of Platform State, databases and files independently.
- Record any accepted loss of automatic rollback or deployment availability.
- Leave excluded workloads and their data untouched until separately retired.

## Acceptance and limits

The source review identifies implementation gaps. It does not establish that
a migration has succeeded. No production service was changed and no native
Application was installed as part of this assessment. A restore rehearsal,
native isolation tests and a complete deployment test remain necessary.

Before accepting a release, verify public certificate renewal, route precedence,
Application authentication, source deployment, database creation and import,
background work, data restore and recovery after restart. For native apps,
also verify non-root identity, child-process limits, filesystem permissions,
authenticated WebSockets and credential access under the dedicated account.

External control-plane policies must be verified separately where they affect
the selected deployment. This repository document intentionally contains no
private inventory or record of those policies.
