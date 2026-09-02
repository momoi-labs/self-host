# Process runtime on s6 — implementation plan

**Date:** 2026-09-02
**Decisions going in:** the Platform becomes a controller, Applications gain a **runtime**, and host processes are supervised by **s6** — on the Host, never in a container.

Background: [controller-model.md](./controller-model.md) and [non-container-applications.md](./non-container-applications.md). Why s6 over launchd+systemd generation, over a supervisor of our own, and why not in a container, is argued there.

## The three decisions this plan closes

### 1. Who runs `s6-svscan`

Bootstrap writes and loads **one** OS unit whose only job is `exec s6-svscan <scandir>` — a launchd agent on macOS, a systemd user unit on Linux. That is roughly 40 lines of OS-specific code written once, not per Application; everything above it stays single-path. The supervision tree then outlives the daemon: restarting `self-host serve` does not disturb a single Application.

Rejected: `s6-svscan` as a child of the daemon (a Platform restart kills every Application — exactly what a supervision tree exists to prevent). Deferred: running the daemon *itself* as a service in that tree. Attractive (crash-restart for free), but it changes the install story, so it is a separate conversation.

### 2. Where the tree lives

```
~/.config/self-host/            # compose::platform_config_dir(), already the Platform's directory
├── traefik-dynamic/            # existing route projection (ADR-0009)
└── services/                   # new: s6 scan directory, owned entirely by the Platform
    ├── .s6-svscan/             # svscan's own control directory
    └── app-<id>/               # one per process Application, keyed by id (ADR-0008)
        ├── run                 # #!/bin/sh — env, then exec the Operator's command
        ├── notification-fd     # "3" — readiness, via s6-notifyoncheck
        ├── data/check          # readiness probe: TCP connect to the app's port
        └── log/
            ├── run             # exec s6-log n20 s1000000 T ./main
            └── main/           # current + rotated files → this is `self-host logs`
```

Same id, same naming discipline as containers: `app-<id>` mirrors `sf-app-<id>`, so a rename never touches the tree.

### 3. How s6 gets onto the Host

1. **Now:** detect. Bootstrap looks for `s6-svscan` in `PATH`; the process runtime is offered only when it is present, and the CLI prints `brew install s6` or the distro package otherwise. No release-surface change, no vendoring commitment.
2. **Later:** vendor. `.goreleaser.yml` already builds the four targets `install.sh` resolves; s6 links statically, so the binaries ride along in the archive into `/usr/local/libexec/self-host/`. Its own ADR, when the friction justifies it.

## The runtime trait, mapped to s6

| Trait method | Container runtime (today) | Process runtime (s6) |
| --- | --- | --- |
| `ensure` | pull/build + `docker run` | write the service directory → `s6-svscanctl -a <scandir>` → `s6-svc -wU -T 30000 -U <dir>` |
| `observe` | `container_running` + `restart_count` | `s6-svstat -o up,ready,pid,updownfor,exitcode <dir>` — one line, space-separated, no scraping of prose |
| `stop` | `docker stop` | `s6-svc -wd -D <dir>` — `-D` writes `./down`, so it stays down across a rescan |
| `logs` | `docker logs -f` | tail `log/main/current` (s6-log rotates; the SSE endpoint is unchanged) |
| `upstream` | `http://sf-app-<id>:80` | `http://host.docker.internal:<port>` — the mechanism the admin route already uses (`src/tls.rs:172`) |
| remove | `docker rm -f` | `s6-svc -xd`, then delete the directory and rescan |

`desired_state` from the controller plan lands exactly on `s6-svc -U` / `-D`: the Operator's intent is a database column, and `./down` is its projection on disk.

### What a generated service looks like

```sh
# services/app-k7m2p9x4qr8t/run          (0755, generated — do not edit)
#!/bin/sh
exec 2>&1
cd /home/seba/projects/t3code
export PORT=3000
export NODE_ENV=production
exec s6-notifyoncheck -d -w 1000 -n 30 -- npx t3@latest

# services/app-k7m2p9x4qr8t/data/check   (0755)
#!/bin/sh
exec nc -z 127.0.0.1 3000

# services/app-k7m2p9x4qr8t/log/run      (0755)
#!/bin/sh
exec s6-log n20 s1000000 T ./main
```

- `exec 2>&1` sends stderr down the same pipe, so `self-host logs` shows both streams, like Docker does.
- `s6-notifyoncheck` gives readiness for a daemon that knows nothing about s6: it polls `data/check` until it exits 0, then fires the notification on the fd named in `notification-fd`. **The route is published only after `ready`** — stricter than the container path, which publishes on "container is running".
- Env comes from `application_env`, so `apps env set` works for process Applications: rewrite `run`, then `s6-svc -r`.
- Plain `/bin/sh`, no execline required.

## Build order

### PR 1 — `AppRuntime` trait + external runtime (S)

No s6 yet. This is the seam everything else lands in, and it already ships something useful.

- `src/runtime.rs` (new): the trait, `Observed`, and a registry resolving a record to its runtime.
- `src/db.rs`: `runtime TEXT NOT NULL DEFAULT 'container'`, `upstream TEXT` — same `ADD COLUMN IF NOT EXISTS` idiom as line 231.
- `src/routes.rs:49`: `render()` takes the upstream from the runtime instead of hardcoding `sf-app-<id>:80`.
- `src/apps.rs`: `finish_deploy`, `remove_application`, `reconcile` go through the trait; `src/main.rs:50` gains `--upstream`.
- `src/bootstrap.rs:144`: Traefik gets `host.docker.internal:host-gateway` (already broken on plain Linux Docker today).
- ADR-0013 "Applications have a runtime"; **Runtime** in `CONTEXT.md`, and the "run as a Docker container" line in the Application entry rewritten.

**Done when:** `apps add --name t3code --upstream 3000` publishes an Operator-run process at `t3code.home.lan` over HTTPS, no container is created, and the image/path paths are unchanged.

### PR 2 — reconcile becomes a loop (M)

- `src/controller.rs` (new): ticker (10s) + `tokio::sync::Notify`, a serial pass, per-Application exponential backoff capped around 5 minutes.
- Every action idempotent: "a container with this id running this image exists", not "run a container". One test asserts that a pass over an unchanged world performs zero actions.
- Deploys stay synchronous in this PR — no API change, only self-healing. A container killed by hand comes back.

**Done when:** `docker rm -f` on an Application container is repaired within one tick, and the route survives a wiped `traefik-dynamic/`.

### PR 3 — s6 substrate (M)

Still no Applications on it: get the tree up and prove it on both platforms.

- `src/s6.rs` (new): scandir layout, service-directory writer (atomic — write to a temp dir, `rename`, then `s6-svscanctl -a`), and thin wrappers over `s6-svc` / `s6-svstat`.
- `src/bootstrap.rs`: detect `s6-svscan`; write and load the launchd agent / systemd user unit; report s6 in `bootstrap_status` so the console can say why the process runtime is unavailable.
- Manual check first, before any of this is written: hand-build one service directory on macOS and on Linux, run `npx t3@latest` under it, and confirm `s6-svstat -o up,ready,pid` and `s6-log` give the console everything `docker logs` gives today.

**Done when:** `self-host init` on a Host with s6 leaves a running `s6-svscan` that survives a Platform restart, and says something actionable on a Host without it.

### PR 4 — `ProcessRuntime` (M)

- `apps add --name t3code --exec "npx t3@latest" --port 3000 [--workdir DIR]`, and `runtime = 'process'`.
- Generate `run`, `data/check`, `notification-fd`, `log/run` from the record plus `application_env`; `ensure`/`observe`/`stop`/`logs` per the mapping table.
- Publish the route on `ready`, not on `up`.
- `apps env set` rewrites `run` and restarts the service; `apps remove` takes the service down and deletes the directory.
- Reject an `--exec` deploy when s6 is absent, with the install hint — not a stack trace.

**Done when:** t3code runs under the Platform, survives `kill -9`, streams logs through `self-host logs t3code`, and comes back after a Host reboot.

### PR 5 — `desired_state` and async deploy (M)

- `desired_state` column, `apps start` / `apps stop` for both runtimes (`s6-svc -U/-D`, `docker start/stop`).
- `POST /apps` returns `202`; the console polls or takes an SSE event instead of blocking on a pull.
- Document the surprise plainly: without `desired_state = stopped`, the controller *will* bring back what the Operator stopped by hand.

## Risks worth naming

- **The vendoring question returns.** "Install s6 first" is friction on a project whose pitch is one binary. Detection keeps it honest for now; if it hurts, vendor rather than live with it.
- **macOS is the weak leg.** s6 is packaged there, but far less exercised than on Linux. Prove the tree by hand on both before PR 3 is written — that check is the go/no-go for this whole plan.
- **Readiness probing needs a probe.** `nc` is not guaranteed present; a small `self-host probe tcp 127.0.0.1:3000` subcommand is a better `data/check` than depending on the Host's netcat flavour.
- **Two projections to keep consistent.** Routes and service directories are both derived from the database; the controller must rewrite both from scratch on every pass so neither can drift into being the source of truth.
- **Contributor unfamiliarity.** s6's docs are terse. Generated `run` scripts should carry a "generated by self-host, do not edit" header, and `docs/` should hold a one-page cheat sheet of the five commands we actually use.

## Still open

- Does the Platform daemon eventually become a service inside its own tree? Elegant, changes the install story — decide after PR 4.
- Port allocation: does the Operator pass `--port`, or does the Platform assign one and export it as `PORT`? Assigning is nicer, but not every app honours `PORT`.
- Do `source` (`'image'`/`'path'`) and `runtime` both stay? Recommendation: yes — `source` is meaningless for the process and external runtimes, and one column cannot honestly hold two ideas.

## References

- s6 — [overview](https://skarnet.org/software/s6/), [s6-svc](https://skarnet.org/software/s6/s6-svc.html), [s6-svstat](https://skarnet.org/software/s6/s6-svstat.html), [s6-log](https://skarnet.org/software/s6/s6-log.html), [s6-notifyoncheck](https://skarnet.org/software/s6/s6-notifyoncheck.html)
- [s6 — Homebrew formula](https://formulae.brew.sh/formula/s6)
