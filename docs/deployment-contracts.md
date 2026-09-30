# Deployment contracts

An Application's record says how it runs (`runtime`), whether Consumers reach
it by Hostname (`publication`) and how its Variables reach a Compose project
(`variable_delivery`). This page is the contract for those fields as shipped,
followed by the shapes later slices are expected to add. The second half is
proposed and not implemented; the code answers only what the first half
describes.

## What is implemented

### The record

| Field | Values | An old row reads | A new row reads |
| --- | --- | --- | --- |
| `runtime` | `{"kind": "container"}` or `{"kind": "native", ...}` | `container` | `container` |
| `publication` | `{"kind": "web"}` or `{"kind": "unpublished"}` | `web` | `web` |
| `variable_delivery` | `"broadcast"` or `"referenced"` | `broadcast` | `referenced` |

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
and `variable_delivery`. Every Application response carries all three. A
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

## Proposed shapes, not implemented

Everything below is a proposal for a later slice. None of it is in the code,
and a request that sends any of it today is ignored or refused as an unknown
field.

### W1: a route rule

Today a route is a set of Hostnames and one loopback target per Application.
W1 adds path routing. A route rule is:

```json
{"hostname": "blog.example.invalid", "path_prefix": "/api",
 "target": "127.0.0.1:20002", "strip_prefix": true}
```

`hostname` matches the request's Hostname; `path_prefix` matches the start
of the path; `target` is a loopback address on the Host; `strip_prefix` says
whether the prefix is removed before forwarding (`false` preserves it). When
several rules match, the longest `path_prefix` wins; the Application's plain
Hostname route is the rule with prefix `/`. The proxy owns matching, the
Application record owns its rules, and the route table is still rebuilt from
the records at start (ADR-0019).

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
