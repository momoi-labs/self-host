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

1. Installs the `self-host` binary to `/usr/local/bin`, creating that
   directory where the Mac does not have one.
2. Chooses a runtime. If `docker info` already works, it keeps what is there
   and says so: Docker Desktop and OrbStack start with a login session, so
   they do not meet the requirement. Otherwise, or with
   `SELF_HOST_RUNTIME=colima`, it installs Colima, the Docker CLI and the
   Compose plugin with Homebrew.
3. Writes a Colima template with `vmType: vz`, `mountType: virtiofs`, the
   Operator's home directory as a writable mount, and `network.dns` set to
   Cloudflare. Without that mount the VM shares nothing of the Mac and every
   bind mount — an Application's data directory, a Compose project — is an
   empty directory inside the VM. An existing profile is patched in place.
   DNS runs natively on the Host and needs no VM port forwarding.
4. Writes `~/.colima/_lima/_config/override.yaml` so Lima ignores guest port
   53. The Colima guest runs `dnsmasq`, and Lima republishes a guest listener
   on the same port of the Mac — `limactl` ends up holding `TCP *:53` with no
   container publishing anything, and the Platform cannot serve DNS. Colima
   has no setting for this and turning its host resolver off does not help;
   the Lima override is where it belongs. An override the Operator wrote by
   hand is left alone, with a message saying what it needs.
5. Installs `/Library/LaunchDaemons/dev.momoi.self-host.colima.plist`: a
   LaunchDaemon that runs `colima start --foreground` as the Operator's user
   (`UserName`), at boot (`RunAtLoad`), kept alive by launchd. A LaunchDaemon
   runs before any login; running it as the Operator keeps the Docker
   context, the socket and the mounted home directory the Operator sees.
6. Runs `self-host init`, which brings up the Platform Infra and detects the
   LAN address. `SELF_HOST_IP` overrides the detection.
7. Writes `/etc/resolver/<suffix>` so the Mac resolves its own Hostnames.
8. Runs `self-host trust-ca`, so the Host's own browser opens
   `https://admin.<suffix>` without a warning. The System Keychain refuses
   this over SSH, where nobody can authorize it; the installer then warns and
   carries on, and the command can be run again from Terminal on the Mac.
   Each Consumer trusts the same CA once with `self-host trust-ca --from
   admin.<suffix> --fingerprint <sha256>`, using the fingerprint
   `self-host init` printed.
9. Installs `/Library/LaunchDaemons/dev.momoi.self-host.plist`: the Platform,
   `self-host serve`, as the Operator's user, at boot, kept alive. The daemon
   starts DNS from `dns.json`, opens its state directory and serves the API,
   none of which needs Docker. Bringing the Platform Infra up runs alongside;
   a Docker that is still starting delays HTTPS and Applications, not the
   console.

Logs land in `~/Library/Logs/self-host/`. `sudo launchctl print
system/dev.momoi.self-host` shows the daemon's state.

## How the pieces come back after a reboot

| Layer | Brought back by |
| --- | --- |
| Colima VM and Docker daemon | `dev.momoi.self-host.colima` LaunchDaemon |
| Platform Infra containers | Docker's `unless-stopped` restart policy, and `self-host serve` as a fallback |
| DNS | The Platform daemon, before Docker is available |
| Platform state | Files under `~/.config/self-host/state/`; the daemon reads them at startup |
| The Platform | `dev.momoi.self-host` LaunchDaemon |
| Application containers | Docker's `unless-stopped` restart policy; the Platform sets it on every Compose service that has none |
| Application routes | `self-host serve` republishes every running Application's route on start |
| Stopped Applications | Stay stopped: `unless-stopped` does not restart a stopped container, and the Platform withdraws the route |

## Starting over

Validation is a loop, and a half-installed Host is what makes the next run
hard to read. `uninstall.sh` puts the Mac back where the installer found it:

```sh
curl -fsSL https://raw.githubusercontent.com/momoi-labs/self-host/main/uninstall.sh | bash
```

It removes both LaunchDaemons, every Platform and Application container,
volume and network, `~/.config/self-host`, the CA in the System Keychain,
`/etc/resolver/<suffix>`, the binary and the logs. It asks first, and
`SELF_HOST_FORCE=1` skips that.

The Colima VM stays, because rebuilding it costs minutes and holds nothing of
the Platform. `SELF_HOST_DELETE_VM=1` removes it too, for a run that has to
prove the VM comes up from nothing. Homebrew and its packages are never
touched, and a Consumer that trusted the CA still trusts it — remove it there
as well.

Run it while the Docker runtime is up. Without Docker the containers and
volumes cannot be removed, and the script says so instead of pretending.

## Known risks to validate first

These are the things the design assumes and nobody has yet seen work on
the Mac. They are listed in the order to check them.

1. **Colima under a LaunchDaemon, before login.** Lima's `vz` driver uses
   Virtualization.framework from a process with no window server. If the VM
   does not start, `colima.log` says why; the fallback is `vmType: qemu`.
2. **Native UDP and TCP 53 reachable from the LAN.** From another machine,
   run `dig @<host-ip> hermes.home.lan` and repeat with `+tcp`. Check an
   external name too, such as `dig @<host-ip> example.com`. Repeat with the
   Docker runtime stopped; DNS must still answer. The Platform listens on
   every interface here, not on the LAN address alone: as the Operator, macOS
   allows port 53 on the unspecified address and refuses it on a named one
   (ADR-0017). So nothing else on the Mac may hold 53 — `lsof -nP -iTCP:53
   -iUDP:53` names whoever does if the Platform cannot bind it.
3. **Ports 80 and 443 reachable from the LAN**, not only from the Mac. The
   Platform binds them itself, on the unspecified address for the same reason
   it binds 53 there: as the Operator, macOS allows a privileged port on the
   unspecified address and refuses it on a named one (ADR-0019). Nothing else
   on the Mac may hold them; `lsof -nP -iTCP:80 -iTCP:443` names whoever
   does.
4. **Bind-mount ownership.** The Hermes container `chown`s `/opt/data`. On a
   virtiofs mount that may be refused. If Hermes logs a permission error,
   set `PUID` and `PGID` in its environment to the Operator's `id -u` and
   `id -g`.
5. **Nothing in the VM reaches back.** The console is served from the
   Platform's own process, and an Application is reached at a Host port it
   publishes on loopback, so no container needs a route to the Mac.

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
