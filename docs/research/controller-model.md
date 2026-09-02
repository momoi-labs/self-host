# The Platform as a controller

**Date:** 2026-09-02
**Research question:** What changes if the Platform stops *performing deploys* and starts *converging the Host on a declared state* — and how does a non-container Application become just another runtime under it?

Follow-up to [non-container-applications.md](./non-container-applications.md). Case study: `npx t3@latest`.

## Why this line fits the codebase

Half of the controller model is already here, by accident of good decisions:

- **The database is the source of truth.** `reconcile()` (`src/apps.rs:417`) already rebuilds Traefik's dynamic directory from `applications` at boot, and ADR-0009 states outright that the route files are "a projection of the database".
- **Intent is recorded before the world is touched.** `prepare_deploy_*` writes a `pending` row and `finish_deploy` does the work afterwards (`src/apps.rs:365`). That split is spec-then-actuate under another name.
- **Identity survives change.** ADR-0008: Applications are keyed by a surrogate id, so a rename never touches Docker. A controller needs exactly this — a stable key to diff against.
- **The actuator is already behind a trait.** `DockerRuntime` (`src/docker.rs:180`) is faked in tests, so a second implementation costs nothing structural.

What is missing is the loop. Today convergence happens exactly twice: inline in the HTTP request, and once at boot. Between two boots the Host can drift as far as it likes — a container killed by hand stays `running` in `apps list`, and nothing brings it back.

## How it would work

One *spec* (what the Operator asked for), one *observation* (what the Host is actually doing), and a loop that closes the gap. Runtime becomes a property of the Application, not an assumption of the code.

```
Operator ──▶ HTTP API ──▶ [ spec in PostgreSQL ]        ← the only thing a request writes
                                   │
                            (notify + ticker)
                                   ▼
                          ┌─────────────────┐
                          │   Controller    │  diff(spec, observed) → actions
                          └────────┬────────┘
                     ┌─────────────┼─────────────┐
                     ▼             ▼             ▼
              DockerRuntime  ProcessRuntime  ExternalRuntime      ← actuators
              (container)    (host process)  (route only)
                     └─────────────┼─────────────┘
                                   ▼
                            RouteStore (Traefik files)
                                   │
                                   ▼
                        [ status written back to PG ]
```

1. `POST /apps` validates, writes the spec row, returns `202` with the Application id. No Docker call in the request path.
2. The write pokes the controller through a `tokio::sync::Notify`; a ticker (say 10s) pokes it anyway, so a lost notification only costs latency.
3. Each pass lists the specs, asks each runtime what it observes, and produces actions: pull/build, create, recreate, remove, publish route, withdraw route.
4. Actions are idempotent and safe to repeat — that is the whole contract. "A container with this id running this image exists", rather than "run a container".
5. The outcome goes back to the row: observed status, `last_error` as the existing `ErrorReport` cause chain (ADR-0010), and a retry with backoff instead of a dead `failed` row.

**Where a non-container app lands:** it is a spec with `runtime = 'external'` (nothing to actuate, route only) or `runtime = 'process'` (spawn and supervise). The controller does not care; it asks the runtime to converge and writes down what it saw. Same Hostname rules, same TLS, same console.

Single Operator, single Host: no leader election, no leases, no cross-process work queue. The controller is one `tokio` task running a serial pass. Resist importing the rest of the Kubernetes vocabulary — ADR-0001 already rejected the cluster shape.

## Options at a glance

| Option | Effort | What it buys | Self-heals drift | Fits one PR |
| --- | --- | --- | --- | --- |
| **1** — Runtime trait, imperative deploy | S | Non-container Applications become a first-class concept | no | yes |
| **2** — Continuous reconcile loop | M | Observed status, self-healing, retries with backoff | yes | yes, if the runtime work lands first |
| **3** — Spec/status split with generations | L | Honest "is it what I asked for?", clean multi-runtime, console conditions | yes | no — 3–4 PRs |
| **4** — Multi-target controller (remote Hosts) | XL | Applications on other LAN machines from one Platform | yes | no |

## Option 1 — Make the runtime a trait, keep deploys imperative (S, start here)

The smallest change that makes the controller story *possible* without committing to the loop yet.

```rust
#[async_trait]
pub trait AppRuntime: Send + Sync {
    fn kind(&self) -> &'static str;                  // "container" | "external"
    async fn ensure(&self, spec: &ApplicationRecord) -> Result<(), RuntimeError>;
    async fn observe(&self, spec: &ApplicationRecord) -> Result<Observed, RuntimeError>;
    async fn stop(&self, spec: &ApplicationRecord)   -> Result<(), RuntimeError>;
    async fn logs(&self, spec: &ApplicationRecord)   -> Result<LogStream, RuntimeError>;
    /// Where Traefik should send traffic for this Application.
    fn upstream(&self, spec: &ApplicationRecord) -> String;
}
```

`ContainerRuntime` wraps today's `DockerRuntime` verbatim; `ExternalRuntime` is nearly empty: `ensure` is a no-op, `observe` is a TCP probe, `logs` returns the existing "nothing to stream" notice, `upstream` is the stored host:port.

**Work**

- `src/db.rs` — `runtime TEXT NOT NULL DEFAULT 'container'` and `upstream TEXT`, added with the same `ADD COLUMN IF NOT EXISTS` idiom as line 231. Keep `source` (`'image'`/`'path'`) as *how the image is obtained*; it is orthogonal to *what runs it*.
- `src/runtime.rs` (new) — the trait, `Observed { running, restarts, detail }`, and a registry resolving a record to its runtime.
- `src/routes.rs:49` — `render()` takes the upstream from the runtime instead of hardcoding `sf-app-<id>:80`.
- `src/apps.rs` — `finish_deploy`, `remove_application` and `reconcile` talk to `AppRuntime`; `DeployWork` stays as is for the container runtime.
- `src/lib.rs` / `src/main.rs` / `console/app.js` — a runtime column in the API response and the list; `--upstream` on `apps add`.
- `src/bootstrap.rs:144` — Traefik gets `host.docker.internal:host-gateway` (already needed for the admin route on plain Linux Docker).
- Docs: ADR-0013 "Applications have a runtime; container is one of them", plus **Runtime** in `CONTEXT.md`.

**Trade-offs**

- Every later option is a change *inside* this seam, not a rewrite of the call sites.
- Test cost is low: `FakeDocker` becomes `FakeRuntime` with the same behaviour.
- Nothing self-heals yet; drift is still invisible between boots.
- One genuine risk: `observe()` tempts you to make `list_apps` hit Docker per row. Keep the stored status as the answer for now.

## Option 2 — Run the reconcile loop continuously (M, the actual controller)

Promote `reconcile()` from a boot-time settle to a background task, and let it own convergence.

```rust
// src/main.rs, after the store and runtimes are built
let controller = Controller::new(store.clone(), runtimes.clone(), routes.clone());
tokio::spawn(controller.run(Duration::from_secs(10)));   // ticker + Notify

// each pass, per Application
match (spec.desired, runtime.observe(&spec).await) {
    (Running, Ok(o)) if o.running  => routes.publish(&spec)?,       // idempotent
    (Running, Ok(_))               => runtime.ensure(&spec).await?, // gone → bring it back
    (Stopped, Ok(o)) if o.running  => runtime.stop(&spec).await?,
    (_,       Err(e))              => record_and_back_off(&spec, e),
}
```

The HTTP handler goes down to: validate → write spec → notify → `202`. Deploy stops blocking on a `docker pull`, which also removes the "the request timed out but the pull is still going" ambiguity the current `pending` row exists to paper over.

**Work**

- Option 1, plus `src/controller.rs`: pass function, ticker, `Notify`, per-Application exponential backoff (cap around 5 minutes), structured logging per action.
- Status columns become observation output: `status` is what the last pass saw, with `last_seen_at`. `pending` keeps its meaning — "no pass has converged this spec yet".
- A `desired_state` column (`running` | `stopped`) — this is what finally makes `apps stop` / `apps start` expressible without new imperative endpoints.
- Console: the app row shows observed vs desired, and `202` means the UI polls (or takes an SSE event) instead of waiting on the POST.
- Tests: a pass over a fake runtime that reports "not running" recreates it; a pass over an unchanged world performs zero actions (the idempotency guarantee, worth an explicit test).

**Trade-offs**

- Kill a container by hand and the Platform brings it back — the property that makes it a PaaS rather than a deploy script.
- A crashed process under a future `ProcessRuntime` is handled by the same code path, for free.
- Deploy becomes asynchronous, so CLI and console need a "waiting for it to come up" affordance; a bad image now fails *after* the command returned.
- The loop can fight the Operator: a hand-stopped container comes back unless `desired_state` is set. That surprise has to be documented, not just implemented.
- Backoff and log volume need care — a permanently broken image must not produce a pull attempt every 10 seconds forever.

## Option 3 — Split spec and status, with generations (L)

Option 2 plus an honest answer to "is the Host running what I asked for?". The row gains `generation` (bumped on every spec write) and a JSONB `status` holding `observed_generation` and conditions.

```json
{
  "observed_generation": 7,
  "conditions": [
    { "type": "Accepted",   "status": true,  "at": "..." },
    { "type": "Programmed", "status": true,  "at": "..." },
    { "type": "Ready",      "status": false, "reason": "ImagePullBackOff",
      "message": "manifest unknown", "at": "..." }
  ]
}
```

**Trade-offs**

- `observed_generation < generation` = "your change has not landed yet" — a real answer for the console and for `apps list`, instead of a status string that means three different things.
- Conditions separate the two failure classes the current single `status` conflates: the route is published but the workload is broken, versus the workload is fine but the Hostname is not being served.
- Natural home for per-runtime differences (an external Application is never `Ready` by the Platform's doing, only `Programmed`).
- Touches the API response shape, the CLI table and the whole console; 3–4 PRs and a migration for existing rows.
- The vocabulary drifts toward Kubernetes, which ADR-0001 deliberately walked away from. Worth it only if the conditions are actually rendered — otherwise it is ceremony.

## The process runtime's backend: s6, init systems, or our own (M)

Once a `ProcessRuntime` exists, the controller still needs something that keeps a host process alive between passes — a 10-second loop is convergence, not supervision.

| Backend | Same on macOS + Linux | Logs | Observe | New host dependency |
| --- | --- | --- | --- | --- |
| **s6** ([skarnet.org/software/s6](https://skarnet.org/software/s6/)) | yes — one mechanism | `s6-log`, rotation included | `s6-svstat` | yes, but small and packaged |
| systemd + launchd | no — two backends | journalctl / log file | two status parsers | none, already on the Host |
| Own supervisor in the daemon | yes | direct pipes | in-process | none |

### Why s6 fits this codebase unusually well

- **Its interface is a directory, and the Platform already actuates through directories.** A service is a `run` script in a scan directory; `s6-svscan` notices it and supervises it. That is the same contract as the Traefik dynamic directory in ADR-0009 — "the filesystem is a projection of the database" — so the process runtime needs no new architectural idea, just a second projection.
- **It splits supervision from convergence cleanly.** `s6-supervise` restarts a crashed process in milliseconds; the controller only decides *whether the service should exist* and reads back state. Our loop stays declarative and never becomes a babysitter.
- **It answers the three questions the runtime trait asks.** `ensure` → write the service directory and rescan; `observe` → `s6-svstat`; `stop` → `s6-svc -d`; `logs` → the `log/` service's output. `desired_state` maps onto `s6-svc -U`/`-D` exactly.
- **Readiness is real.** `notification-fd` gives "the process is actually accepting connections", which is what the route should wait on before publishing — better than the "container is running" approximation used today.
- **One mechanism on both platforms.** The MVP supports macOS and Linux (`install.sh`); s6 is POSIX-portable (Homebrew ships it, distros ship it, `s6-linux-init` is the only Linux-only piece and we are not PID 1). Generated launchd plists *and* systemd units would mean two code paths, two log stories and two failure modes.

### Why s6 does *not* go in a container

Tempting, since every other piece of Infra is a container — but it does not work, for three independent reasons:

- **A supervisor supervises its own children, in its own namespaces.** `s6-supervise` inside a container spawns `npx t3@latest` in that container's PID, mount and user namespaces: no Operator home, no git worktrees, no SSH keys, no local toolchain. That host coupling is the entire reason the process runtime exists.
- **On macOS it is not even theoretically possible.** Docker Desktop is a Linux VM, so "the host" seen from a container is the VM, not the Mac. No combination of `--privileged`, `--pid=host` or `nsenter` reaches a macOS process — which removes the main argument for s6 over launchd+systemd in the first place.
- **Lifetime is wrong.** The supervision tree should outlive the Platform and Docker itself. In a container, a `docker restart`, a daemon upgrade or a Docker crash takes every host Application down with it.

On Linux you could force it with `--privileged --pid=host -v /:/host` plus `nsenter`. That is root on the Host with extra steps and worse ergonomics, and it still leaves macOS unsolved.

Two nearby things that *are* legitimate, and should not be confused with this: `s6-overlay` *inside an Application image* that needs several processes, and using a container purely as a *delivery* mechanism (`docker create` + `docker cp` to drop the static Linux binaries onto the Host — works, but only for Linux).

### Should Platform Infra run under s6?

No, on both readings of the question.

- **Infra as native processes** would mean asking the Operator for PG 18, CoreDNS and Traefik binaries on macOS and Linux, giving up pinned versions, the managed volume and the two-network isolation of ADR-0012 — and Traefik's Docker provider needs Docker present anyway. ADR-0002 and ADR-0003 made that trade; s6 does not change it.
- **Infra containers supervised by s6** puts two supervisors on one process: Docker's `restart: unless-stopped` and `s6-supervise` both try to own it, which produces double-start races. Making it coherent means stripping the restart policy and dropping the compose file (`src/compose.rs`, ADR-0011), plus reboot-ordering against the Docker daemon on Linux. The gain — uniform logs and readiness ordering — does not pay for that.

The rule to write into the ADR: **one supervisor per process domain.** Docker owns containers (Infra and container Applications), s6 owns host processes. Ambiguity here is what makes systems like this hard to debug.

The exception worth revisiting later is `self-host serve` itself: it is a host process, so it belongs to s6's domain by that rule, and it is the one system service with a real gap today — no crash restart, no start on boot, no log retention. It changes the install story, so it is a separate decision.

### Packaging it

1. **First (PR-sized):** detect it. Bootstrap checks for `s6-svscan` in `PATH`; the process runtime is offered only when it is there, and the CLI prints `brew install s6` or the distro package otherwise.
2. **Later:** vendor per-platform binaries into the release. `.goreleaser.yml` already builds the same four targets `install.sh` resolves, so s6 rides along in the archive and installs into `/usr/local/libexec/self-host/`. s6 links statically, so this stays a copy rather than a dependency chain.
3. **Never:** an `sf-system-s6` Infra container.

**Recommendation:** s6, if the vendoring question can be answered — the directory-as-interface shape is the one the Platform already committed to, and it is the only option that stays single-path across macOS and Linux. Fall back to launchd/systemd generation if shipping a third-party binary is unacceptable. Do not write our own supervisor.

## Option 4 — Multi-target controller (XL, post-MVP)

The runtime gains a *target*: the Host the Platform runs on, or another LAN machine reached over SSH or a remote Docker endpoint. The controller loop is unchanged; only the actuator learns to work at a distance, and routes point at `http://<target-ip>:<port>` instead of a container name.

- This is the payoff of the whole line: the same loop that fixes a dead container on `localhost` fixes one on the machine in the closet.
- Credentials, connectivity, partial failure and per-target backoff are a different class of problem. Explicitly out of MVP ("Multi-host / cluster" in [docs/mvp-scope.md](../mvp-scope.md)).
- Listed only so options 1–3 are not designed in a way that forecloses it: keep the target out of the container name and inside the runtime, and this stays reachable.

## Recommended path

1. **PR 1 (option 1).** `AppRuntime` trait + `ContainerRuntime` + `ExternalRuntime`, `runtime`/`upstream` columns, `--upstream` on the CLI, route render via the runtime, the host-gateway fix, ADR-0013. Publishes t3code on the LAN and pays for itself immediately.
2. **PR 2 (option 2, half).** Move `reconcile` into a task with a ticker and make every action idempotent — but keep deploys synchronous. Drift is now healed; nothing about the API changes.
3. **PR 3 (option 2, rest).** `desired_state`, `202` on deploy, backoff, console polling. The behavioural change worth its own release note.
4. **PR 4 (option 1 again, third runtime).** `ProcessRuntime`, backed by s6 — detailed in [s6-process-runtime.md](./s6-process-runtime.md).
5. **Later.** Option 3 if the console grows a real Application detail view; option 4 only after multi-host leaves the out-of-scope list.

## Decisions to close before PR 1

- **`runtime` vs `source`:** keep both (what runs it / where the image comes from), or collapse into one column? Recommendation: keep both — `source` is meaningless for external and process runtimes, and a single column would have to encode two ideas.
- **Glossary:** "Application" in `CONTEXT.md` currently says "run as a Docker container". That sentence is the decision this whole line changes; it needs new wording, plus a **Runtime** entry.
- **Word choice:** `CONTEXT.md` tells us to avoid Kubernetes jargon. "Controller" and "reconcile" already live in the code and are worth keeping; "spec", "conditions" and "generation" should not appear until option 3 is actually built.
