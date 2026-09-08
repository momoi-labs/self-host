# Running Applications that are not containers

**Date:** 2026-09-02
**Research question:** The Platform runs Applications as Docker containers. What are the least-effort options for publishing a host-native workload on the LAN — and which of them fits in a single PR?

Case study: [pingdotgg/t3code](https://github.com/pingdotgg/t3code), started as `npx t3@latest`.

## Where the Platform stands today

- MVP decision #4 ([docs/mvp-scope.md](../mvp-scope.md)) fixes the Application runtime to a **Docker container**, from an image pull or a local build. `applications.source` records which (`'image'` | `'path'`).
- Routing is already decoupled from Docker: ADR-0009 makes Traefik read a per-Application file, rendered by `render()` in `src/routes.rs:49`, hardcoded to `http://sf-app-<id>:80` (`APP_CONTAINER_PORT`, `src/apps.rs:6`).
- There is **already a non-container upstream in production**: the admin route points at the daemon on the host, `http://host.docker.internal:<OPERATOR_API_PORT>` (`src/tls.rs:172`). TLS, DNS and the Hostname rules come for free for anything routed this way.
- Everything else about an Application (status, logs, env, restart count, remove) goes through `DockerRuntime` and `container_name_for(id)`.

The routing/DNS/TLS half of the problem is already solved. The open question is only *who runs the process* and *how much of the Application lifecycle the Platform still owns*.

## Case study: what t3code actually needs

- Node >= 22.16, started as `npx t3@latest`; serves HTTP on a local port, not 80.
- It is an *agent harness control surface*: it drives coding agents against the Operator's real working trees, credentials and CLIs on the Host. Sandboxing it inside a container is against its purpose — it would need the host filesystem, the Docker socket and the Operator's tool credentials mounted in.

This matters for the ranking below: option A is the cheapest change to the repo, but it is the **wrong shape for t3code specifically**. Options B–D are what serve the case study.

## Options at a glance

| Option | Effort | Who runs the process | Logs | Env | Restart / health | One PR? |
| --- | --- | --- | --- | --- | --- | --- |
| **A** — Container recipe | XS | Platform (Docker) | yes | yes | yes | yes |
| **B** — External upstream (`--upstream`) | S | Operator | no | no | reachability only | yes |
| **C** — Generated service unit | M | Platform → systemd / launchd | via journal/file | yes | yes | tight |
| **D** — Native process runtime | L | Platform (own supervisor) | yes | yes | yes | no |

## Option A — Wrap it in a container recipe (XS, docs only)

No Platform change at all. Ship a documented recipe directory the Operator deploys with the existing build path:

```
self-host apps add --name t3code --path ./examples/t3code
```

The recipe is a Dockerfile on a Node base image that runs the app and listens on port 80.

**Work:** `examples/<app>/Dockerfile` plus a README section, "Applications that ship without an image". Note the port contract: the container **must** listen on 80 (`src/apps.rs:6`); apps with a fixed port need a shim (`socat`/nginx) or an env override in the recipe.

**Trade-offs**

- Zero risk, zero new domain concepts, keeps MVP decision #4 intact.
- Does not solve t3code: an agent harness needs the host, not a sandbox. Escaping the sandbox (host FS + `docker.sock` mounts, `--network host`) buys the container's downsides with none of its benefits.
- Every non-container app becomes the Operator's Dockerfile maintenance problem.

Use when the workload is an ordinary web app that merely lacks a published image (static site, plain Node/Go service). Not for host-coupled tools.

## Option B — External Application: the Platform routes, the Operator runs (S, recommended)

Add a third Application `source`: `'external'`. The Application record carries an **upstream** instead of an image; deploy writes the Traefik route and nothing else — no pull, no build, no container. The Operator starts the process however they like (`tmux`, launchd, systemd, by hand).

```
# the Operator runs the process on the Host, listening on :3000
self-host apps add --name t3code --upstream 3000
self-host apps add --name t3code --upstream http://192.168.1.20:8080   # another LAN box
```

A bare port means "this Host", rendered as `http://host.docker.internal:<port>` — the mechanism the admin route already uses.

### Work, mapped to files

1. `src/db.rs` — `ALTER TABLE applications ADD COLUMN IF NOT EXISTS upstream TEXT` in the migration chain (the file already uses that idiom, e.g. line 231); add `upstream` to `ApplicationRecord`, the upsert and the three SELECTs.
2. `src/apps.rs` — `prepare_deploy_from_upstream()` next to the image/path pair; `DeployWork::None` so `settle_deploy` skips Docker and lands on `running`; validate the upstream (bare port `1-65535`, or an `http(s)://host[:port]` URL).
3. `src/routes.rs:49` — `render()` branches on `source`: container name + 80, or the stored upstream. Everything else (rule, aliases, TLS, entryPoint) is unchanged.
4. `src/apps.rs` — `remove_application()` skips `remove_container` for external; `reconcile()` skips the container check.
5. `src/lib.rs` — `upstream` on `DeployApplicationRequest` / `UpdateApplicationRequest`; the `(image, path)` match at line 256 becomes a three-way source selection with a clear "exactly one of" error; `nothing_to_stream()` (line 544) returns "this Application runs outside the Platform; logs are the Operator's"; the status list (line 283) reports reachability instead of `restart_count`.
6. `src/main.rs:50` — `--upstream` on `apps add`, mutually exclusive with `--image`/`--path`.
7. `console/app.js` — a source selector on the deploy form; render the upstream where the image column is today (lines 143, 192, 306, 370).
8. `src/bootstrap.rs`, `infra_containers()` — add `extra_hosts: host.docker.internal:host-gateway` to Traefik. **Pre-existing gap:** Docker Desktop supplies that name, plain Linux Docker does not, so the admin route at `src/tls.rs:172` is already fragile there. Fixing it once serves both.
9. Docs: `docs/adr/0013-external-applications.md` amending MVP decision #4; `CONTEXT.md` gains **External Application** and keeps the "Application" entry honest; README quickstart line.

### Tests

- `routes::render` for an external record points at the upstream and keeps alias rules.
- Deploy with `upstream` creates a `running` Application and touches `FakeDocker` zero times.
- Remove withdraws the route and does not try to remove a container.
- Deploy with two sources set is a 400.

### Trade-offs

- Smallest change that genuinely publishes t3code on the LAN with Hostname + TLS + DNS.
- Zero new runtime surface: no supervision, no process ownership, no log plumbing.
- Bonus: fronts anything already running on the LAN (a NAS UI, a printer, a Raspberry Pi).
- The Platform no longer owns the lifecycle: no logs, no env injection, no restart. `apps list` must not claim `running` without qualification — report reachability.
- Amends a closed MVP decision, so it needs an ADR, not just code.

## Option C — Platform generates a service unit (M)

Option B plus supervision delegated to the OS: `apps add --exec "npx t3@latest" --port 3000` writes a systemd user unit (Linux) or a launchd agent (macOS), enables it, and routes to the port. Restart policy, boot persistence and log retention become the init system's job.

**Work:** everything in option B, plus a `ProcessUnit` writer with two OS backends, unit naming from the Application id, and `start`/`stop`/`remove` wiring; `logs` shells out to `journalctl --user -u` or tails the launchd log file; env from `application_env` rendered into the unit.

**Trade-offs**

- Near parity with containers (restart, boot, logs) without writing a supervisor.
- Two OS code paths, each with its own failure modes; user-scope vs system-scope, and on macOS the daemon's own permissions bleed into the Application's.
- Bigger than one comfortable PR, and it only pays off once option B's model exists. Build it *on top of* B, not instead of it.

## Option D — Native process runtime inside the Platform (L)

A `ProcessRuntime` trait mirroring `DockerRuntime`: the daemon spawns the child, allocates a port, captures stdout/stderr into the existing log stream, injects `application_env`, restarts on exit, reconciles on daemon start, and kills on remove.

**Trade-offs**

- One consistent Application model: every command works the same for both runtimes.
- The Platform becomes a process supervisor — orphan reaping, zombie children, restart backoff, port allocation, log rotation, and a daemon restart that must not kill or double-start the workload.
- Multi-PR, and it re-fights problems systemd/launchd already solved. Only worth it if the Platform must run on hosts where neither is usable.

## Recommendation

1. **Now, one PR:** option B. Additive, has an in-tree precedent (the admin route), leaves the Docker path untouched, and it is the option that actually publishes t3code on the LAN.
2. **Same PR, small:** the Traefik `host-gateway` fix — a real Linux bug today, not new scope.
3. **Follow-up:** option C behind `--exec`, once the external source has proved itself.
4. **Keep option A as documentation** for apps that merely lack an image; it is not a substitute for B.
5. **Skip option D** unless a target Host has no usable init system.

### Acceptance criteria for the option B PR

- `self-host apps add --name t3code --upstream 3000` returns a `running` Application with Hostname `t3code.<suffix>`.
- A Consumer on the LAN reaches the host-native process over HTTPS at that Hostname; aliases work as they do for containers.
- No container is created; `docker ps` shows nothing new.
- `apps remove t3code` withdraws the route and deletes the record, without a Docker error.
- `logs` answers with an explicit notice instead of failing.
- Image and path deploys are unchanged; existing tests pass untouched.

### Out of scope for that PR

- Starting, stopping or supervising the process (options C/D).
- Env injection into an external Application.
- Health checks beyond a reachability probe on `list`.
- Per-Application upstream TLS (external upstreams are plain HTTP on the Host).

## Follow-up

The controller framing and the supervision backend are worked out in [controller-model.md](./controller-model.md) and [s6-process-runtime.md](./s6-process-runtime.md).
