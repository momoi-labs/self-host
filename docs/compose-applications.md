# Compose Applications

An Application can be defined as a Docker Compose file supplied through the
console, the API or `self-host apps add --compose-file` (ADR-0014). The
Platform does not run the file as written. It reads a documented subset,
refuses anything outside it by name, and renders a project of its own. This
page is that subset and what the rendered project looks like.

## What the Platform adds

For an Application with id `k3n8qz4v2x1p` and a service called `hermes`:

| Concern | What the Platform does |
| --- | --- |
| Project name | `sf-app-k3n8qz4v2x1p`. The same prefix a single-container Application uses. |
| Container names | `sf-app-k3n8qz4v2x1p-hermes`, one per service. The file's `container_name` is replaced. |
| Networks | New Applications use their own network. Existing Applications keep `sf-apps` until the Operator changes their network policy. See private connectivity below. |
| Labels | `sf.app.id`, `sf.app.name` and `sf.app.service` on every container. |
| Restart policy | `unless-stopped` when the service has none, so the Application returns after a reboot. |
| Environment | Application Variables fill the file's `${VAR}` references. Applications created before this get every Variable on every service instead; see Variables below. |
| Files | `~/.config/self-host/apps/<id>/compose.yml`, rendered again on every deploy. |

The rendered file is the one `docker compose` runs. The Operator's file is
kept verbatim on the Application's row and is what the console edits.

## Routing

The Hostname routes to one service and one container port, the web target.
The web port is the port the service listens on inside its container, and
Consumers only ever see the Hostname on 443. The console asks for both. Left
empty, the web service is the first one that publishes a port and the web port
is the container side of its first publication. The resolved pair is recorded,
so the route never guesses again; changing either rewrites the route without
touching Docker.

The Platform's proxy runs on the Host, not on `sf-apps`, so it cannot reach a
container by name. The web target is published on a Host port bound to
loopback instead (ADR-0019): reachable by the proxy, and by nothing on the
LAN. The port is chosen once and kept, and only the web target's service gets
one.

## Published ports

`ports` are passed through and published on the Host directly, without
HTTPS. A host port the Platform Infra already uses is refused:
53, 80, 443, 3721 and 15432. The Hostname route does not need a published
port; keep `ports` only for things that are not the browser, such as an API
another client talks to.

## Persistent storage

Named volumes keep their project names, `sf-app-<id>_<volume>`. To attach
an existing volume deliberately, name it and mark it external:

```yaml
services:
  db:
    image: postgres:17
    volumes:
      - data:/var/lib/postgresql/data
volumes:
  data:
    external: true
    name: synthetic-existing-data
```

The volume must already exist. A missing external volume fails deployment;
the Platform never substitutes empty storage or copies data. `name` without
`external: true`, `external` without `name`, driver settings, labels and
unknown options are refused. Stop the previous writer before mapping its
volume to an Application.

Short mounts use `SOURCE:TARGET[:ro|rw]`. Long mounts accept `type` (`bind`
or `volume`), `source`, `target` and `read_only`. Bind mounts also accept
`bind.create_host_path`. Other mount options are refused by name.

| Bind source | Host path |
| --- | --- |
| `~` or `~/.example` | `~/.config/self-host/apps/<id>/data/.example` |
| `./config.json` | `~/.config/self-host/apps/<id>/data/config.json` |
| `/absolute/path` | The given Host path |

Existing files remain files; directories remain directories. A missing
long-syntax source fails unless `bind.create_host_path: true` explicitly
requests a directory. Missing short-syntax sources keep the previous behavior
and create directories. Create file sources before deploying. Relative paths
cannot leave the Application's data directory, including through symlinks.
Special files are refused. Docker receives `create_host_path: false` after
the Platform prepares each source.

```yaml
volumes:
  - type: bind
    source: ./config.json
    target: /etc/example/config.json
    read_only: true
    bind:
      create_host_path: false
```

Restart, redeploy and ordinary removal keep named volumes and bind data.
Removal runs Compose without `--volumes`. Deleting stored data is a separate
Operator action.

## Private connectivity

`network_policy` is explicit Platform State. Old rows read as
`{"kind":"shared"}` and keep the shared `sf-apps` bridge. New Applications
use `{"kind":"private","consumers":[]}`. Their own bridge permits outbound
connections but does not join other Applications.

An unpublished Compose Application can grant access to named container
Applications:

```json
{"network_policy":{"kind":"private","consumers":["synthetic-consumer-id"]}}
```

The provider joins an internal Docker network, `sf-private-<provider-id>`.
Only its declared consumers join that network. The provider cannot declare
`ports` or `extra_hosts`, has no Web Target and creates no public listener.
Consumers address a provider service by its container name,
`sf-app-<provider-id>-<service>`. Grant changes connect or disconnect the
consumer's existing containers, including stopped containers, without
starting them. Removing an Application with grants or references is refused
until those connections are removed.

Credentials remain explicit Application Variables. A network grant copies
no Variable, creates no database role and invents no password. Give each
consumer its own database credentials, then set them only on that consumer.
Referenced Variable delivery still limits which Compose services receive
those values. Database provisioning and credential rotation belong to P2.

The native endpoint adapter validates a dedicated non-root Application
Account and publishes only on `127.0.0.1`. It adds the provider's own bridge
because Docker cannot publish a port from an internal-only network. That
bridge is not shared with other Applications. The adapter has no active
Operator API path before N3. Loopback limits access to the Host; it does not isolate Host
accounts. Database authentication must still reject clients without a
consumer's credentials. Native API creation and native consumer grants remain
disabled.

## Variables

Application Variables are set with `self-host apps env set` or on the
console. The file reaches them with Compose's own syntax, and the Platform
resolves the references itself when it reads the file (ADR-0030). Docker
Compose never interpolates the rendered project: every `$` in it is written
as `$$`, so the daemon's environment has no road into a container.

| Written | Read as |
| --- | --- |
| `$VAR`, `${VAR}` | The Variable's value. Empty when it is not set. |
| `${VAR:-default}` | `default` when `VAR` is unset or empty. |
| `${VAR-default}` | `default` when `VAR` is unset. |
| `${VAR:?message}` | Refused when `VAR` is unset or empty; the refusal carries `message`. |
| `${VAR?message}` | Refused when `VAR` is unset. |
| `${VAR:+alt}` | `alt` when `VAR` is set and not empty, otherwise empty. |
| `${VAR+alt}` | `alt` when `VAR` is set, otherwise empty. |
| `$$` | A literal `$`. |

A default or an alternative may itself hold a reference, `${A:-${B}}`. A `$`
followed by anything else, such as `$1` or an unclosed `${`, is refused and
the refusal names the text.

An unset Variable reads as empty, the way Compose reads an unset shell
variable. To insist on one, write `${VAR:?}`. That check runs when the
Platform renders the project to run it (a deploy, a start, a restart), not
when the file is saved, so the file can be saved first and the Variable set
after. The daemon's own environment is never consulted: a variable the
Platform was started with is not a Variable of the Application.

A bare `environment` entry, `- VAR` in a list or `VAR:` with no value in a
mapping, takes the Variable's value when it is set and is dropped when it is
not, as Compose does with the shell environment.

### Delivery

An Application created from now on gets its Variables by reference: a
Variable reaches a service only where the file names it, and a service that
never mentions `${SECRET}` never sees it.

Applications created before this keep broadcast delivery. Every Variable is
copied into every service's `environment`, whether the file mentions it or
not, so nothing they relied on is removed silently. Move the references into
the file, then switch with `{"variable_delivery": "referenced"}` in
`PUT /apps/id/{id}`. The rendered project changes, so the switch redeploys
the Application.

## Supported service keys

`image` (required), `command`, `entrypoint`, `environment`, `ports`,
`volumes`, `extra_hosts`, `restart`, `depends_on`, `deploy`, `healthcheck`,
`labels`, `working_dir`, `user`, `expose`, `stop_grace_period`, `init`.
`container_name` is accepted and replaced.

`extra_hosts` takes the short syntax only, one `HOST:ADDRESS` string per
list item. Docker resolves the address `host-gateway` to the Host, so
`host.docker.internal:host-gateway` reaches the Host by name, which the
container otherwise cannot. A plain `name:address` entry adds no reach:
it only lets a name resolve to an address the container could already
open a socket to, which is what a TLS-verified HTTPS call to a LAN
service needs.

Top level: `services`, `volumes` (managed or explicitly mapped external volumes),
`version` (ignored), `name` (ignored).

YAML anchors (`&common`) and aliases (`*common`) are resolved when the file
is read, and `<<` merge keys are applied. Keys starting with `x-`, at the top
level or on a service, are Compose extension fields. They are dropped after
the merge, which is what makes them the usual home of a shared block:

```yaml
x-common: &common
  restart: always
services:
  web:
    <<: *common
    image: nginx
```

Anything else is refused, and the refusal names the key: `build`,
`network_mode`, `networks`, `privileged`, `cap_add`, `devices`, `env_file`,
`extends`, `secrets`, `configs` and so on. This is not a judgement on those
features. It is the line between what the first Application needed and
what nobody has tested on this Platform.

## State

An Application is `running`, `stopped`, `failed` or `pending`.

The row records what the Operator asked for. The console shows that word
checked against Docker: an Application on record as running whose service
has exited is shown as `failed`, with the service name, its exit code and
its restart count as the reason. The `services` list on the API carries
every container's state and its Docker health status when a healthcheck is configured. Logs come from every service of the project,
prefixed by service name, and still stream after a container has exited so
the reason it exited can be read.

`stop` stops the containers and withdraws the route. A stopped Application
stays stopped across a Platform restart and a Host reboot; `start` brings it
back. `restart` restarts the containers in place.

A restart or a redeploy can pull newer images first, so a moving tag such as
`:latest` picks up a new release. Each image a registry serves is pulled;
images built on the Host are not. A restart that pulls recreates the
containers from the new images, and a failed pull leaves them running as they
were. A redeploy that pulls brings the project up, which recreates the
services whose image changed. The `pull_newer_images` setting picks the
default, and `{"pull": true}` or `{"pull": false}` in the body of
`POST /apps/id/{id}/restart` or `PUT /apps/id/{id}` overrides it once.

## Example

The official Hermes file, as accepted:

```yaml
services:
  hermes:
    image: nousresearch/hermes-agent:latest
    restart: unless-stopped
    command: gateway run
    volumes:
      - ~/.hermes:/opt/data
    environment:
      - HERMES_DASHBOARD=1
      - HERMES_DASHBOARD_BASIC_AUTH_USERNAME=operator
      - HERMES_DASHBOARD_BASIC_AUTH_PASSWORD=change-me
    deploy:
      resources:
        limits:
          memory: 4G
          cpus: "2.0"
```

Web service `hermes`, web port `9119`: the port the dashboard listens on
inside the container. Consumers open `https://hermes.<suffix>`, resolved by
the Platform DNS and served by the Platform itself, which forwards to the
loopback port that container port is published on. The file publishes no port
of its own; it has no `ports` at all. See
[hermes.md](hermes.md) for the complete workflow.
