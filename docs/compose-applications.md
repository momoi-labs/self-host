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
| Networks | Every service joins the project's own network and `sf-apps`. Nothing joins `sf-system` (ADR-0012). |
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

Short-syntax `volumes` only, `SOURCE:TARGET[:MODE]`.

| Source | Meaning on the Host |
| --- | --- |
| `data` (a name) | A named volume, `sf-app-<id>_data`, created by Compose. |
| `~` or `~/.hermes` | `~/.config/self-host/apps/<id>/data/.hermes`. |
| `./x` | `~/.config/self-host/apps/<id>/data/x`. |
| `/absolute/path` | Passed through. With Colima, only paths under the Operator's home are visible to the VM. |

Relative paths cannot leave the data directory. The Platform creates the
directories it bind-mounts before `up`, so they belong to the Operator and
not to root.

`MODE` is `ro` or `rw`. The SELinux labels `z` and `Z`, the Docker Desktop
hints `cached`, `delegated` and `consistent`, and `nocopy` mean nothing on
this Host and are refused by name. Long-syntax mounts are refused as well.

A top-level `volumes` entry declares a named volume and nothing more. The
Platform creates it under the project name, and that is the whole contract:
`external: true`, `driver`, `driver_opts`, `name` and `labels` are refused,
as in "volume 'data': 'driver' is not supported yet".

Data survives every deploy, start, stop and restart: `docker compose up`
recreates containers, not volumes. Removing an Application removes its
containers and project network and keeps its named volumes and data
directory on the Host.

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

Top level: `services`, `volumes` (named volumes without `external`),
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
every container's state. Logs come from every service of the project,
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
