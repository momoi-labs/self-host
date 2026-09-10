# Remote development environments

Investigation recorded on September 9, 2026, for running one development
environment per person on the Host, with [T3 Code](https://t3.codes) inside it
and reached through the Platform's proxy. The Operator drives it from a phone,
a tablet, a laptop or the Workstation; the code and the agent always stay on
the Host.

Unlike most research here, this one was tested. A container ran T3 Code 0.0.40
on ARM64 Linux, an iPhone paired with it, and an agent task survived the client
being force-quit and the phone rebooting. What was **not** exercised: the Host
itself, Colima, and the Platform's routing. The test ran on the Operator's
Workstation under OrbStack, and every claim about the proxy below comes from
reading the code rather than from a request.

This changes nothing in the Platform and proposes no ADR. A development
environment would be an ordinary Compose Application. See the
[vision](../vision.md), which already states that T3 Code is a possible
development tool and not part of the Platform.

## Findings

### T3 Code runs on ARM64 Linux

The npm package ships a `resource-monitor` helper for `darwin-arm64`,
`darwin-x64`, `linux-x64` and `win32-x64`, with nothing for `linux-arm64`.
Since a container on Apple Silicon is ARM64 Linux, this looked like a blocker.
It is not. On `node:24-bookworm` at `linux/arm64`, the server started, applied
its 49 SQLite migrations, answered HTTP 200 and never mentioned the missing
helper. `package.json` restricts neither `os` nor `cpu`.

### The server binds loopback only by default

`t3 serve` defaults to `--mode desktop`, which keeps loopback defaults and
picks a random port. That reads as a hard limitation and is not one. A remote
environment needs an explicit mode, interface and port:

```sh
t3 serve --mode web --host 0.0.0.0 --port 3773 \
   --no-browser --base-dir /data/t3home
```

A fixed `--port` is required, not optional. A random port breaks the route on
every restart. `--base-dir` (equivalently `T3CODE_HOME`) is where the
environment's own state goes, which makes it the volume mount point.

Inside a container, `--host 0.0.0.0` binds the container's network namespace
only. Published to loopback on the Host, that is exactly the Web Target shape
of [ADR-0019](../adr/0019-embedded-http-proxy.md).

### Pairing tokens expire in five minutes

The token printed at startup has a five minute TTL. On a Host that stays up for
weeks it is useless within the hour, and the client reports it as an invalid
credential rather than an expired one. Enrolment has to mint a token
deliberately:

```sh
t3 pair --base-dir /data/t3home --ttl 2h --label iphone
```

If the Platform ever presents development environments in the console, this is
the step that belongs there. The Operator should not need a shell to add a
phone.

### The agent CLI must share the machine with the server

T3 Code's documentation requires the coding agent CLI to be installed and
authenticated on the same machine as the server. The pilot showed the negative
case: with no Claude CLI present, the server logs `Claude Agent CLI health
check failed` on every start and offers no Claude model. Server, CLIs and
credentials are therefore one unit, and the container boundary has to contain
all three.

Codex CLI 0.154.0 has no device-authentication flag; its login serves a
callback on `localhost:1455`, so a headless container needs that port reachable
from wherever the browser runs. `CODEX_HOME` is honoured and is inherited by
the processes the server spawns, so the credential can live on the volume.

### Work survives the client leaving

An agent task was started from the iPhone, the app was force-quit and the phone
was restarted. A sampler inside the container recorded output every fifteen
seconds. Times are the container's UTC clock.

```
[00:24:17] rel=11 procs=7    app force-quit
[00:24:32] rel=12 procs=7
[00:25:17] rel=13 procs=7    phone rebooting
[00:26:02] rel=14 procs=7    client returning
```

The agent's own timestamps agree: 00:24:25, 00:25:04, 00:25:52 and 00:26:31,
through the window when the phone was off. The seven agent processes never
dipped, and reconnecting restored the thread with everything done in the
meantime. T3 Code documents terminal scrollback as server-side state restored
on reconnect, which matches what was observed for threads.

### The session reaper is not a risk

The server logs `provider.session.reaper.started` with a thirty minute
inactivity threshold and a five minute sweep. This was initially read as a
limit on leaving a task running unattended. It is not: it reaps sessions that
are idle, and an agent working through a task is not idle. Threads are in
SQLite, so the worst case for a genuinely idle session is a slower first
response while a provider process restarts.

### Supervision inside a container

On Linux, `t3 service install` uses a systemd user service with lingering
enabled. A container has no systemd, so that path does not apply. Docker's
restart policy is the supervisor, and `--init` reaps the agent CLI's orphaned
children.

## What this needs from the Platform

### Nothing, for the transport

`src/proxy.rs` serves connections with upgrades and bridges the upgrade in both
directions, keeping the `Connection` and `Upgrade` headers that make up the
handshake. WebSockets pass. Together with Host-native DNS
([ADR-0017](../adr/0017-host-native-dns.md)), the local CA and the device setup
page added for [issue #78](https://github.com/momoi-labs/self-host/issues/78),
the access path already exists.

The native desktop and mobile clients connect over plain HTTP on the LAN, which
is what the pilot used, so CA trust is not a prerequisite. Routing through the
Platform is still worth doing: terminating TLS is what lets the hosted web
client work as well, so the Hostname route serves both kinds of client instead
of one.

Tailscale was considered and dropped. T3 Code supports it natively through
`serve --tailscale-serve` and `pair --tailscale`, and it remains available if
the Operator's existing VPN proves inadequate for access from outside the LAN.
It is not needed for this design.

### A Hostname implies an Application

`src/routes.rs` derives a route's target from the Application record's
`web_target_port`, on loopback. The Platform has no concept of a route to
something it does not manage, so an environment that wants a Hostname has to be
a Compose Application.

That is acceptable, with one caveat and one mitigation. The caveat is
circularity: the Platform would manage the environment in which the Platform is
developed. The mitigation is that it is smaller than it looks. The container
carries `restart: unless-stopped`, so a stopped Platform leaves the environment
running and only withdraws its Hostname. What is lost is reachability, because
the Web Target is published on loopback only. Declaring an explicit `ports`
entry alongside it keeps a way in for exactly the moment the Platform is down.
`ports` are passed through to the Host, as
[compose-applications](../compose-applications.md) documents.

## The shape of an environment

Two parts that must not be confused.

| | Image | Volume |
| --- | --- | --- |
| Contents | T3 Code, agent CLIs, Git, language toolchains | Repositories, T3 Code state, SSH key, provider credentials |
| Scope | Shared, one recipe for everyone | One per person, never shared |
| Lifetime | Replaced on toolchain updates | Outlives container recreation; the thing to back up |
| Credentials | Never | Provisioned once on first boot |

Nothing is configured on every start. The first boot provisions the volume, and
every boot after that finds it already populated. A credential in the image
would be a credential shared between people, which defeats the separation the
per-person environment exists to provide.

### Credentials

SSH can be fully automatic. Generate the key inside the container on first
boot, into the volume, where it stays. Use the same key for commit signing with
`gpg.format = ssh`, so there is one key and one enrolment. With `gh`
authenticated, registration is scriptable through `gh ssh-key add` and `gh
signing-key add`.

Agent forwarding is the obvious alternative and does not work here. The server
is long-lived and headless; a forwarded agent dies with the client that carried
it, which is precisely the disconnect this design exists to survive.

Provider authentication needs one browser step per person per provider. After
that the token sits on the volume and refreshes itself. Making it zero-touch
means API keys instead of subscription logins, which is a billing decision
rather than a technical one, and it should be settled before a second person is
enrolled. Subscriptions are per person, so two people means two logins.

For secrets that must come from outside rather than be born in the container,
[1Password service accounts](onepassword-secrets.md) were researched separately.
For SSH keys, generating in place is simpler than distributing.

## Risks not yet addressed

**SQLite on a shared filesystem.** T3 Code keeps state in SQLite with a write
ahead log. The installer mounts the Operator's home into the Colima VM over
virtiofs, and SQLite over a shared filesystem has known locking hazards. The
volume must be a named Docker volume on the VM's own disk, never a bind mount
into the Mac's home directory.

**Backups.** A named volume lives inside Colima's disk image, so Time Machine
does not see it. Uncommitted work and thread history would sit outside every
existing backup. Copying a live database with an open write ahead log is not a
backup; the mechanism needs `sqlite3 .backup` plus an archive written into the
Operator's home, which is backed up.

**The memory budget.** The ceiling is not the Mac's RAM but whatever Colima
reserved for its VM. Container limits divide that number rather than expand it,
and [Docker containers have no limits by
default](https://docs.docker.com/engine/containers/resource_constraints/). Two
people running Rust builds compete inside it. Measure before the second
environment exists; the answer may be that the VM needs resizing first.

**Isolation.** Separate volumes are the mechanism. That one environment cannot
read the other's has not been tested, and neither has concurrent building.

**A single Host.** Every unpushed branch and every thread would live on one
machine. Push often, and get the volume archive off the box.

## A gap in the glossary

`CONTEXT.md` has an Operator, Consumers and Applications. A development
environment is none of those: it is not published to Consumers, and its value is
that a person can get inside it rather than that it answers on a Hostname.
Calling it a Compose Application works for a pilot and will chafe.

If this becomes how the Operator develops, the term to resolve is something
like a Workspace beside an Application, with its definition committed to the
repository it serves. That is a decision for after two environments have run
for a few weeks, not now. Noted here so the language gap is on record.

## Sources

Behaviour is as observed in T3 Code 0.0.40 on September 9, 2026, and in the
`t3` npm package at that version. Upstream documentation:
[remote access](https://github.com/pingdotgg/t3code/blob/main/docs/user/remote-access.md),
[install](https://github.com/pingdotgg/t3code/blob/main/docs/user/install.md),
[background service](https://github.com/pingdotgg/t3code/blob/main/docs/user/background-service.md),
[terminal](https://github.com/pingdotgg/t3code/blob/main/docs/user/terminal.md).
Unpinned documentation can change, and an alpha product's behaviour can change
with it.
