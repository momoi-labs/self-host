# The macOS Host

What the installer expects of an Apple Silicon Mac, what it puts there, and
how to validate that the Platform and its Applications return after a
reboot without anyone logging in (ADR-0013).

## Status of validation

Unattended startup has **not yet been validated on the real Mac**. The
design below is what the installer sets up and what needs proving. The
acceptance record at the end is to be filled in on the Host; until it is,
the corresponding items in [mvp-plan.md](mvp-plan.md) stay open.

## Prerequisites

- Apple Silicon Mac on macOS 13 or later, for the `vz` virtual machine type.
- [Homebrew](https://brew.sh), unless a Docker runtime already answers
  `docker info`.
- A user account for the Operator with administrator rights. The installer
  runs as that user and asks for `sudo` where it writes to `/Library` and
  `/etc`.
- A fixed LAN address for the Mac, by DHCP reservation on the router or a
  manual address. The Platform answers every Hostname with it, and the other
  machines point at it as their DNS server.
- **FileVault off**, or the disk otherwise unlocked at boot. With FileVault
  on, the Mac stops at the pre-boot unlock screen and nothing below starts
  until someone types a password. Apple's remote unlock over SSH still needs
  a person. This is a Host condition, not something the Platform can work
  around.
- Energy settings that keep the Mac awake and bring it back after a power
  cut: in System Settings, Energy, enable "Start up automatically after a
  power failure" and "Prevent automatic sleeping when the display is off",
  or `sudo pmset -a sleep 0 autorestart 1`. A closed-lid MacBook on power
  stays awake only with an external display attached or with
  `sudo pmset -a disablesleep 1`.
- Automatic login is **not** required and is not what this relies on.

## What the installer does

```sh
curl -fsSL https://raw.githubusercontent.com/momoi-labs/self-host/main/install.sh | bash
```

1. Installs the `self-host` binary to `/usr/local/bin`.
2. Chooses a runtime. If `docker info` already works, it keeps what is there
   and says so: Docker Desktop and OrbStack start with a login session, so
   they do not meet the requirement. Otherwise, or with
   `SELF_HOST_RUNTIME=colima`, it installs Colima, the Docker CLI and the
   Compose plugin with Homebrew.
3. Writes a Colima template with `vmType: vz` and `mountType: virtiofs`.
   DNS runs natively on the Host and needs no VM port forwarding.
4. Installs `/Library/LaunchDaemons/dev.momoi.self-host.colima.plist`: a
   LaunchDaemon that runs `colima start --foreground` as the Operator's user
   (`UserName`), at boot (`RunAtLoad`), kept alive by launchd. A LaunchDaemon
   runs before any login; running it as the Operator keeps the Docker
   context, the socket and the mounted home directory the Operator sees.
5. Runs `self-host init`, which brings up the Platform Infra and detects the
   LAN address. `SELF_HOST_IP` overrides the detection.
6. Writes `/etc/resolver/<suffix>` so the Mac resolves its own Hostnames.
7. Runs `self-host trust-ca`, so the Host's own browser opens
   `https://admin.<suffix>` without a warning. Each Consumer trusts the same
   CA once with `self-host trust-ca --from admin.<suffix> --fingerprint <sha256>`,
   using the fingerprint `self-host init` printed.
8. Installs `/Library/LaunchDaemons/dev.momoi.self-host.plist`: the Platform,
   `self-host serve`, as the Operator's user, at boot, kept alive. The daemon
   starts DNS from `dns.json`, then waits for Docker, brings the Infra up
   if it is not, waits for PostgreSQL, then serves the API. It logs once a
   minute while waiting for Docker or PostgreSQL.

Logs land in `~/Library/Logs/self-host/`. `sudo launchctl print
system/dev.momoi.self-host` shows the daemon's state.

## How the pieces come back after a reboot

| Layer | Brought back by |
| --- | --- |
| Colima VM and Docker daemon | `dev.momoi.self-host.colima` LaunchDaemon |
| Platform Infra containers | Docker's `unless-stopped` restart policy, and `self-host serve` as a fallback |
| DNS | The Platform daemon, before Docker or PostgreSQL is available |
| The Platform | `dev.momoi.self-host` LaunchDaemon |
| Application containers | Docker's `unless-stopped` restart policy; the Platform sets it on every Compose service that has none |
| Application routes | `self-host serve` republishes every running Application's route on start |
| Stopped Applications | Stay stopped: `unless-stopped` does not restart a stopped container, and the Platform withdraws the route |

## Known risks to validate first

These are the things the design assumes and nobody has yet seen work on
the Mac. They are listed in the order to check them.

1. **Colima under a LaunchDaemon, before login.** Lima's `vz` driver uses
   Virtualization.framework from a process with no window server. If the VM
   does not start, `colima.log` says why; the fallback is `vmType: qemu`.
2. **Native UDP and TCP 53 reachable from the LAN.** From another machine,
   run `dig @<host-ip> hermes.home.lan` and repeat with `+tcp`. Check an
   external name too, such as `dig @<host-ip> example.com`. Repeat with the
   Docker runtime stopped; DNS must still answer. Check for another process
   holding the Host's port 53 if the Platform cannot bind it.
3. **Ports 80 and 443 reachable from the LAN**, not only from the Mac.
   macOS lets a non-root process bind ports below 1024 since 10.14, so the
   Operator's user is enough.
4. **Bind-mount ownership.** The Hermes container `chown`s `/opt/data`. On a
   virtiofs mount that may be refused. If Hermes logs a permission error,
   set `PUID` and `PGID` in its environment to the Operator's `id -u` and
   `id -g`.
5. **`host.docker.internal` from Traefik.** The console route goes to
   `host.docker.internal:3721`. Colima maps it to `host.lima.internal`.

## Acceptance record

Fill in on the Host. Until every row has a date, the issue is open.

| Check | How | Result |
| --- | --- | --- |
| macOS version | `sw_vers` | |
| Runtime version | `colima version`, `docker version` | |
| Reboot, no login | Reboot; from another machine open `https://admin.<suffix>` before logging in | |
| Startup from shutdown | Power off, power on; same check | |
| Hermes returns | Open `https://hermes.<suffix>` after the reboot, log in, send a message | |
| LAN DNS without Docker | Local and external queries over UDP and TCP while Docker is stopped | |
| Two Consumers | Chat from two browsers on two machines | |
| Data survives recreation | Edit the Compose file, save, confirm the previous conversation is still there | |
| Stopped stays stopped | Stop an Application, reboot, confirm it is still stopped and its Hostname does not answer | |
| Failure is visible | Set a bad `command`, save, confirm `failed` with the exit code and the logs | |
