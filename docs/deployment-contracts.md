# Deployment contracts

An Application's record says how it runs (`runtime`), whether Consumers reach
it by Hostname (`publication`) and how its Variables reach a Compose project
(`variable_delivery`). It also owns path rules (`route_rules`) and private
network grants (`network_policy`). This page separates implemented contracts
from proposals for later slices.

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
optional). It is recorded shape only: see the refusal below.

### Invariants

- `unpublished` means an empty `hostname`, empty `aliases`, no `web_service`,
  no `web_port` and no Host port. No Record in the Zone, no route in the
  proxy, no loopback publication in the rendered Compose project. A restart
  never gives it a port.
- `web` is what every Application did before the field existed. A published
  Application with no Host port yet answers `503` (ADR-0019).
- `publication` is chosen at creation. An update that changes it is refused;
  the same value is a no-op.
- `runtime: native` is refused on create and update and records nothing.
- `variable_delivery` may change on update. On a Compose Application the
  rendered project changes with it, so the change reaches Docker.

### Requests and responses

`POST /apps` and `PUT /apps/id/{id}` accept optional `runtime`, `publication`
`variable_delivery`, `route_rules` and `network_policy`. Every Application
response carries these fields. A
request without them behaves exactly as before.

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
 "runtime": {"kind": "native", "account": "sf-app-api",
             "command": ["/opt/api/bin/serve"], "port": 8080,
             "limits": {"memory_bytes": 268435456}}}

501 Not Implemented
{"error": "native execution is not available yet; the Application runtime must be container",
 "caused_by": []}
```

### Refusals

| Request | Status | `error` |
| --- | --- | --- |
| `runtime: native` on create or update | `501` | `native execution is not available yet; the Application runtime must be container` |
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
copy a provider's Variables. Managed database roles and connection references
remain P2. Service responses include Docker's `health` when a healthcheck
exists, including for unpublished Applications.

P1's native endpoint adapter checks the dedicated non-root account and renders
only `127.0.0.1`. It is unavailable through the API until N3. Loopback restricts
reachability to the Host; database authentication still restricts clients.
See [Compose storage and networking](compose-applications.md).

## Native supervision boundary

N2 provides protected s6 services and Linux boot delegation. N1 performs every
Application launch, including readiness commands. The privileged daemon and
s6 runner perform setup; Application commands execute as their dedicated
non-root account. ADR-0031 records this choice and the helper alternative.
Native create and update requests still return `501` until N3.

## Proposed shapes, not implemented

The remaining shapes are proposals for later slices. They do not enable
certificate issuance, native API lifecycle or source builds in this wave.

### W2: certificates by server name

Today one wildcard certificate covers the DNS Suffix. W2 adds a lookup keyed
by server name (SNI): the proxy asks for `blog.example.invalid` and receives
the certificate to present, falling back to the wildcard when none is
recorded for the name. ACME HTTP-01 challenge paths
(`/.well-known/acme-challenge/*`) are answered by the Platform before any
Operator rule, including W1 rules, so a challenge is never forwarded to an
Application. Certificate storage and renewal are the Platform's; an
Application never sees a key.

### N3: from a native definition to a launch

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
is created per launch. The Operator API accepts `runtime: native` only once
this mapping runs; until then the refusal above stands.

### P2: a connection reference

A managed PostgreSQL Application (ADR-0029) hands a consumer its connection
through a reference the Platform resolves into one Variable:

```json
{"database_application_id": "k3n8qz4v2x1p",
 "consumer_application_id": "m7p2xr9wq4tn",
 "variable": "DATABASE_URL"}
```

The Platform creates a role and a database for the consumer on the database
Application, writes the URL into the consumer's `DATABASE_URL` Variable, and
rotates it when asked. The consumer reads a Variable like any other; it never
sees the database's own credentials. Removing the consumer removes the role;
removing the database Application is refused while a reference points at it.

### D2: a Git source

A source build names a repository, a ref, a build context and its build
inputs, and the Platform pins the commit it built (ADR-0029):

```json
{"name": "api", "source": {"git": "https://git.example.invalid/team/api.git",
 "ref": "main", "context": "services/api", "dockerfile": "Dockerfile"}}
```

The response carries `commit` once built. A redeploy builds the pinned commit
unless the request asks for the ref's newer commit, which is reported as a
change from one commit to another, the way a pulled image is.
