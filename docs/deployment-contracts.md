# Deployment contracts

An Application's record says how it runs (`runtime`), whether Consumers reach
it by Hostname (`publication`) and how its Variables reach a Compose project
(`variable_delivery`). It also owns path rules (`route_rules`) and private
network grants (`network_policy`). This page defines the API and persistence contracts.
Managed PostgreSQL connection references are implemented in P2.

Git deploy triggers, health gates and immutable image recovery are documented
in [Deployment triggers and recovery](deployments.md). Automatic Git deployment
is disabled until the Operator enables a trigger for that Application.

## What is implemented

### The record

| Field | Values | An old row reads | A new row reads |
| --- | --- | --- | --- |
| `runtime` | `{"kind": "container"}` or `{"kind": "native", ...}` | `container` | `container` |
| `publication` | `{"kind": "web"}` or `{"kind": "unpublished"}` | `web` | `web` |
| `variable_delivery` | `"broadcast"` or `"referenced"` | `broadcast` | `referenced` |
| `route_rules` | Array of hostname/path rules | `[]` | `[]` |
| `network_policy` | `{"kind":"shared"}` or `{"kind":"private","consumers":[]}` | `shared` | `private` |

A native Runtime carries `account`, `command` (argv), `working_dir` (relative
to the account's home, or absent), `port` (the loopback port when it is a Web
Target) and `limits` (`cpu_percent`, `memory_bytes`, `max_tasks`, each
optional). The account defaults to `sf-app-<id>` when omitted or empty.
An explicit account must match that dedicated name. Native execution requires
Linux with the installed N1/N2 supervisor.

Native `recipe` contains `dependencies` and `setup`. Dependencies use the same
`tool`, `version`, optional npm `allow_builds` and optional `options` fields as
custom images. `options` maps a mise tool option name to its values, such as
`{"extras": ["serve", "ane"]}` on a `pypi:` tool. mise receives one value as a
string and several as an array.
Setup is a list of shell commands executed as the Application Account before
startup. An old record without a recipe reads as an empty one. See
[native Application environments](native-applications.md#application-environment)
for installation, logging and restart behavior.

### Invariants

- `unpublished` means an empty `hostname`, empty `aliases`, no `web_service`
  and no `web_port`. It has no HTTP Web Target, Zone Record, proxy route or
  HTTP Host port. Managed PostgreSQL has one transport exception: an explicit
  Native Application connection publishes a separate PostgreSQL port only on
  `127.0.0.1`, with credentials for that consumer. This creates no HTTP route.
- `web` is what every Application did before the field existed. A published
  Application with no Host port yet answers `503` (ADR-0019).
- `publication` is chosen at creation. An update that changes it is refused;
  the same value is a no-op.
- Runtime kind cannot change after creation. Native requests cannot carry
  container definitions or Git sources.
- A published native Application uses its declared loopback `port` as
  `web_target_port`; the Platform allocates no Docker Host port. An
  unpublished native Application has no port.
- `variable_delivery` may change on update. On a Compose Application the
  rendered project changes with it, so the change reaches Docker.

### Requests and responses

`POST /apps` and `PUT /apps/id/{id}` accept optional `runtime`, `publication`
`variable_delivery`, `route_rules` and `network_policy`. Every Application
response carries these fields. A container request without them keeps the
existing defaults.

```json
POST /apps
{"name": "blog", "image": "nginx"}

202 Accepted
{"id": "k3n8qz4v2x1p", "name": "blog", "hostname": "blog.example.invalid",
 "aliases": [], "image": "nginx", "status": "pending", "source": "image",
 "runtime": {"kind": "container"}, "publication": {"kind": "web"},
 "variable_delivery": "referenced", "services": [], "task_id": "..."}
```

```json
POST /apps
{"name": "worker",
 "compose": "services:\n  worker:\n    image: alpine\n    command: sleep infinity\n",
 "publication": {"kind": "unpublished"}}

202 Accepted
{"id": "...", "name": "worker", "hostname": "", "aliases": [], "image": "alpine",
 "status": "pending", "source": "compose", "compose": "services:\n  worker:...",
 "runtime": {"kind": "container"}, "publication": {"kind": "unpublished"},
 "variable_delivery": "referenced", "services": [], "task_id": "..."}
```

No `web_service`, no `web_port`, no route, no Record. `GET /dns/records`
lists nothing for it.

```json
POST /apps
{"name": "api",
 "runtime": {"kind": "native",
             "command": ["/usr/bin/python3", "-m", "http.server", "8080",
                         "--bind", "127.0.0.1"], "port": 8080,
             "limits": {"memory_bytes": 268435456}}}

202 Accepted
{"id": "...", "source": "native", "status": "pending",
 "runtime": {"kind": "native", "account": "sf-app-<id>",
             "command": ["..."], "port": 8080,
             "limits": {"memory_bytes": 268435456}}, "task_id": "..."}
```

### Refusals

| Request | Status | `error` |
| --- | --- | --- |
| A native request with an invalid identity, command or path | `400` | Native definition validation error |
| An update changing Runtime kind | `400` | Runtime cannot change after creation |
| `publication` on update differs from the record | `409` | `publication cannot be changed after creation yet` |
| `unpublished` with `hostname`, `aliases`, `web_service` or `web_port` | `400` | `invalid Application Hostname: an unpublished Application has no Hostname` |

Errors keep the `{error, caused_by}` shape of ADR-0010.

### Who owns what

- `src/store.rs` defines `Runtime`, `NativeDefinition`, `Publication` and the
  record fields; `src/file_store.rs` persists them with the defaults above.
- `src/apps.rs` holds the invariants: `pending_record`, `prepare_update`,
  `check_compose`, `reconcile` and `web_target_publication` refuse or skip
  what an unpublished Application must not have.
- `src/routes.rs` answers with no Hostnames and no target for an unpublished
  Application, and `publish` withdraws its id. The DNS inventory
  (`src/dns_records.rs`) derives from `routes::hostnames` and so lists
  nothing for it.
- `src/lib.rs` maps the request and response fields and the three statuses.

## Path rules and private connections

`route_rules` contains `{hostname, path_prefix, target, strip_prefix}` objects.
Targets are loopback socket addresses. Segment matching chooses the longest
prefix; `/app` does not match `/apple`. Automatic Hostname and alias routes
keep prefix `/`. An explicit root replaces its owner's automatic root.
Different Applications may own disjoint paths on the same Hostname. Removing
one keeps the other's rules and derived DNS answer.

An omitted list preserves rules on update; `[]` removes explicit rules.
Invalid rules return `400`; duplicate keys return `409`. The management
Hostname and certificate challenge subtree are reserved. A route-only update
with `pull: false` takes effect without Docker work. See [route rules](routing.md)
for prefix behavior, WebSockets and the W2 challenge-handler boundary.

`network_policy` preserves `shared` on old records. New Applications use
`{"kind":"private","consumers":[]}`. Ordinary private Applications keep
outbound networking on their own bridge. An unpublished Compose Application
can grant other container Applications access through its internal network:

```json
{
  "network_policy": {
    "kind": "private",
    "consumers": ["m7p2xr9wq4tn"]
  }
}
```

Consumer ids must exist and cannot repeat or refer to the provider itself.
A provider with grants cannot declare ports or `extra_hosts`. Grant changes
connect or disconnect consumers without starting stopped containers. Removing
a provider or consumer with live references returns `409`; remove its grants
first. A Compose Application can change policy explicitly. Single-container
policy changes are refused in this slice; existing shared policies remain
unchanged.

Credentials remain Variables assigned explicitly to a consumer. Grants do not
copy a provider's Variables. P2 adds managed database roles and connection
references. Service responses include Docker's `health` when a healthcheck
exists, including for unpublished Applications.

P1's native endpoint adapter checks the dedicated non-root account and renders
only `127.0.0.1`. Generic native consumer grants remain unavailable. P2 enables
the adapter for managed database connection references. Loopback restricts
reachability to the Host; database authentication still restricts clients.
See [Compose storage and networking](compose-applications.md).

## Native supervision boundary

N2 provides protected s6 services and Linux boot delegation. N1 performs every
Application launch, including readiness commands. The privileged daemon and
s6 runner perform setup; Application commands execute as their dedicated
non-root account. ADR-0031 records this choice and the helper alternative.
Native API requests use this same path for deploy, lifecycle actions, status
and logs. Missing supervision fails the Task with an actionable error; it
never launches an Application directly or falls back to root. Updates retain
stopped intent. Removal revokes the account and preserves inaccessible data.
See [Native Applications](native-applications.md).

## Certificates by server name

W2 stores its explicit configuration, ACME account credentials, status and
atomic certificate/key bundles under `state/certificates`. Directories are
`0700`; private files are `0600`. With no public configuration, local-CA mode
continues to work. Each configured hostname gets its own HTTP-01 order.

SNI selects an exact public certificate or a local certificate whose SAN
covers the requested name. Unknown SNI, an expired certificate or a configured
public hostname without a valid certificate fails the TLS handshake. A LAN
wildcard cannot cover an unrelated public hostname. A client without SNI uses
the local certificate for setup compatibility.

Renewal begins in the last third of a certificate's lifetime, at most 30 days
before expiry. Failures remain visible and retry hourly; a still-valid
certificate remains served. Successful renewal replaces it in memory without
restarting Applications. `GET /certificates` requires Operator authentication
and returns status, never keys. See [public certificates](public-certificates.md).

HTTP-01 paths (`/.well-known/acme-challenge/*`) precede redirects, management
routes and W1 Application rules. Missing challenges return `404`.

## From a native definition to a launch

A `NativeDefinition` on the record plus the Application's Variables becomes
one `native::launch::LaunchRequest` (the N1 contract):

| `LaunchRequest` field | Comes from |
| --- | --- |
| `application_id` | the record's `id` |
| `account` | `native::identity::AccountName::parse(definition.account)`; by convention `sf-app-<id>` |
| `command` | `definition.command`, refused when empty |
| `working_dir` | the account's home joined with `definition.working_dir`; the home itself when absent; refused outside the home |
| `environment` | the Application's Variables as `(name, value)`; `HOME`, `USER` and `LOGNAME` are refused |
| `limits` | `definition.limits`, a `native::ResourceLimits` |
| `purpose` | `Main` for the process the Hostname routes to; `Hook`, `Build` and `Terminal` for the others |

`definition.port` is the loopback port the Web Target answers on, so the
proxy target is `127.0.0.1:<port>` and no Host port is allocated for a native
Application. The account is provisioned once per Application and the cgroup
is created per launch. Native publication checks that the account owns a
listener bound only to loopback before it publishes the route.

## Managed PostgreSQL connection reference

A managed PostgreSQL Application (ADR-0029) hands a consumer its connection
through a reference the Platform resolves into one Variable:

```json
{"database_application_id": "k3n8qz4v2x1p",
 "consumer_application_id": "m7p2xr9wq4tn",
 "variable": "DATABASE_URL"}
```

The Platform creates a role and a database for the consumer on the database
Application, then applies the URL through the consumer's Task queue. The
consumer reads a Variable like any other; it never sees the database's own
credentials. Disconnect disables the role and removes the managed Variable,
preserving its database. Removing either Application is refused while a live
reference points at it. See [managed PostgreSQL](managed-postgresql.md) for
readiness, import and explicit data-removal behavior.

## Git sources and immutable builds

`POST /apps` accepts `git` instead of an image, Host path, inline Compose or
development definition. Git sources use the container Runtime. `PUT
/apps/id/{id}` accepts the same source fields and `refresh_source`, which
defaults to `false`.

`POST /source/inspect` accepts `{ "git": GitSource }` and returns the resolved
revision, selected ref, detected build files, TCP ports and Compose service
names. It checks out the repository without running its code or building an
image. The response excludes manifest values and credential values.

Creation can send `source_revision` to build that reviewed commit without
permanently pinning the saved source. An update sends `source_revision` together
with `expected_git_revision`. The Platform checks the expected deployed commit
before queueing, before building and before replacing the current release.
This prevents a moved branch from changing the candidate or an old review from
replacing a newer deployment. Existing callers can omit both fields.

Saved provider access uses authenticated `/source/connections` endpoints.
`GET` lists metadata; `POST` validates and saves a name, provider and token,
with provider `github` or `gitlab`. `PUT /{id}` reconnects without
changing credential references; `DELETE /{id}` removes access. `GET
/{id}/repositories?page=1` returns paginated repository metadata. Each connection
includes `authentication`, with `token`, `github-app` or `oauth`. Older records
default to `token`. Tokens remain in private storage, bound to the provider's
HTTPS Git host. Reconnection preserves the connection and credential ids,
authentication method and provider. App and OAuth reconnection also verifies
the original authorized account or installation. Legacy token reconnection
keeps its existing username behavior.

### Provider setup and authorization

| Endpoint | Request | Response |
| --- | --- | --- |
| `GET /source/integrations` | None | GitHub/GitLab configured state and non-secret setup metadata |
| `PUT /source/integrations/gitlab` | `console_url`, `client_id`, `client_secret` | Integration metadata, without the secret |
| `POST /source/integrations/github/register` | `console_url`, optional `organization` | Authorization state and a GitHub manifest form |
| `POST /source/authorization/start` | `provider`, `name`, optional `connection_id` | Authorization state and provider URL |
| `POST /source/authorization/complete` | `state`, optional `code`, numeric `installation_id`, provider `error` | Saved `connection`, updated `integrations`, or next-stage `authorization` |
| `POST /source/authorization/cancel` | `state` | `204`, with the pending state revoked locally |

All these routes require an API key. Mutations record bounded operation
metadata in audit events. The Platform never records authorization states,
codes, client secrets, private keys or provider response bodies in that history.

`console_url` must be the console's browser origin plus `/console/`, without
credentials, query or fragment. HTTPS is required except HTTP loopback for
local development. Replacing an integration used by active App/OAuth connections
is refused so existing Applications cannot acquire unrelated access.

The public `GET /source/authorization/callback` serves only a popup bridge.
It checks the pending state and sends bounded callback data to its stored exact
origin. It performs no token exchange, creates no connection, and accepts no
API key in the query. The page escapes JSON, uses a nonce Content Security
Policy, and sets `no-store` and `no-referrer`. An authenticated parent window
checks the message origin, popup source and state before completing the flow.

Authorization states expire within ten minutes and are single-use. Denial,
malformed completion and cancellation cannot reuse a state. GitLab uses
authorization code with PKCE S256. GitHub first records the selected App
installation, then starts user authorization in the same popup with a new
state. The final exchange verifies the installation through the authorized
user's installation inventory and checks the configured App.

GitHub installation tokens and GitLab OAuth tokens are resolved before both
repository listing and checkout. Renewal keeps the saved credential id.
Concurrent renewal, reconnect and disconnect cannot publish stale credentials
or overwrite newer connection status. Access remains restricted to GitHub.com
or GitLab.com. Manual token access remains available through the existing
credential and connection APIs.

```json
{
  "name": "api",
  "git": {
    "repository": "https://git.example.invalid/team/api.git",
    "git_ref": "main",
    "context": "services/api",
    "dockerfile": "Dockerfile"
  }
}
```

The Application response keeps `source: "git"`, its non-secret `git` inputs
and `git_build` with the pinned commit and immutable image IDs. An old row
reads both fields as absent. A redeploy uses the recorded commit unless the
Operator changes the source selection or explicitly requests the newer ref.
The build Task validates every candidate before replacing the current
Application definition or runtime. A failed build leaves the running release
and routes untouched; the Task records the failure.

Checkout paths cannot escape the repository, including through symlinks.
Compose `build` and `env_file` inputs are materialized from checked checkout
files into an ordinary build-free Compose definition. Registry image and
ordinary Compose workflows retain their behavior and P1 network defaults.

Private checkout and registry credentials use IDs in source inputs. The
Platform stores values in `state/source/credentials` with restricted modes.
Build secrets use separate BuildKit mounts; ordinary build arguments are
recorded configuration. Authenticated `/source/credentials` creation and
listing return metadata only. Repository output never becomes an unrestricted
Host shell command. Dockerfile build commands and Git Application commands
must use explicit non-root users. See [Git source builds](git-source-builds.md).
