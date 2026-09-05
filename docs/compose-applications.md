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
| Environment | Variables set with `self-host apps env set` are added on top of the file's `environment`, on every service. |
| Files | `~/.config/self-host/apps/<id>/compose.yml`, rendered again on every deploy. |

The rendered file is the one `docker compose` runs. The Operator's file is
kept verbatim on the Application's row and is what the console edits.

## Routing

The Hostname routes to one service and one container port, the web target.
The web port is the port the service listens on inside its container, not a
port on the Host: Traefik forwards to it over `sf-apps`, and Consumers only
ever see the Hostname on 443. The console asks for both. Left empty, the web
service is the first one that publishes a port and the web port is the
container side of its first publication. The resolved pair is recorded, so the route never guesses
again; changing either rewrites the Traefik route without touching Docker.

Traefik reaches the service by container name over `sf-apps`, with HTTPS on
the Hostname and no host port involved.

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

Data survives every deploy, start, stop and restart: `docker compose up`
recreates containers, not volumes. Removing an Application removes its
containers and project network and keeps its named volumes and data
directory on the Host.

## Supported service keys

`image` (required), `command`, `entrypoint`, `environment`, `ports`,
`volumes`, `restart`, `depends_on`, `deploy`, `healthcheck`, `labels`,
`working_dir`, `user`, `expose`, `stop_grace_period`, `init`.
`container_name` is accepted and replaced.

Top level: `services`, `volumes` (named volumes without `external`),
`version` (ignored), `name` (ignored).

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
      - HERMES_DASHBOARD_BASIC_AUTH_USERNAME=seba
      - HERMES_DASHBOARD_BASIC_AUTH_PASSWORD=change-me
    deploy:
      resources:
        limits:
          memory: 4G
          cpus: "2.0"
```

Web service `hermes`, web port `9119`: the port the dashboard listens on
inside the container. Consumers open `https://hermes.<suffix>`, resolved by
CoreDNS and served by Traefik, which forwards to that port over `sf-apps`.
No host port is published; the file has no `ports` at all. See
[hermes.md](hermes.md) for the complete workflow.
