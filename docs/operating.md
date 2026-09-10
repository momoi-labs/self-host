# Operating the Platform

Everything the Operator configured lives in one directory on the Host:

```
~/.config/self-host/
├── state/                 # authoritative: settings, credentials, Applications
│   ├── platform.json
│   ├── platform.lock
│   └── applications/<application-id>/
│       ├── application.json
│       └── compose-<digest>.yaml
├── dns.json               # what DNS serves, under the address policy
├── config.json            # this machine's CLI credentials
├── certs/                 # the Platform CA and its certificates
└── apps/<application-id>/ # generated Compose projects and Application data
```

Only `state/` and `certs/` are authoritative. Everything else is rebuilt from
them: routing on every deploy, and every running Application's route again on
`self-host serve`. `dns.json` is written by `self-host init` and carries the
DNS Suffix plus the address policy — which addresses to `include` and which to
`exclude`. The addresses themselves are not stored: the daemon scans the Host's
interfaces every 30 seconds and publishes every LAN address that is actually
up, so a restore onto a Host with a different address needs no edit at all.
`--host-ip` on `init` pins a candidate into `include`; it is still served only
while an interface has it.

The daemon is the only writer. `state/platform.lock` is held for as long as it
runs, and a second `self-host serve` against the same directory is refused
rather than allowed to interleave writes. Read-only commands still work while
it runs.

Directories are `0700` and files `0600`. `state/` holds the Operator's API key
and every Application's environment, secrets included. Do not loosen it, and do
not copy it anywhere world-readable.

## Backup

Copy `~/.config/self-host/`. `state/` and `certs/` are the parts that cannot be
regenerated: the Application IDs, the Operator's credentials and the CA the
Consumers on the LAN already trust.

Take the copy with the daemon stopped. A running daemon can commit a change
between two files being read, and the copy would then hold half of it:

```bash
# Linux
systemctl --user stop self-host   # or however you supervise it
tar czf self-host-backup.tar.gz -C ~/.config self-host
systemctl --user start self-host

# macOS
sudo launchctl bootout system/dev.momoi.self-host
tar czf self-host-backup.tar.gz -C ~/.config self-host
sudo launchctl bootstrap system /Library/LaunchDaemons/dev.momoi.self-host.plist
```

**This is not a backup of your Applications' data.** Docker named volumes and
bind mounts outside `apps/<id>/` hold what your Applications wrote, and none of
it is in this archive. Back those up separately.

## Restore

Stop the daemon, unpack the archive over `~/.config/self-host/`, and start it
again. Application IDs, credentials and the CA come back as they were, so
containers keep their names, Consumers keep trusting the CA and the CLI keeps
its key. Routing and rendered Compose projects are rebuilt on start; a restore
onto a Host with no Docker still gives you a readable console.

Restoring onto a Host with a different LAN address needs nothing special: the
daemon scans and publishes what the interfaces have. If an address you pinned
with `--host-ip` is gone for good, re-run `self-host init` or remove it from
`include` in `dns.json`.

## Upgrading from a Platform that used PostgreSQL

Platforms up to 0.2.0-beta.2 kept their state in a `sf-system-db` container.
This version cannot read that database, and it will not start an empty
installation beside it. `self-host init` stops and tells you so.

Export what is in there before clearing it. There is no automatic import:

```bash
docker exec sf-system-db pg_dump -U selfhost selfhost > self-host-state.sql
```

The dump holds the `platform_state` table (DNS Suffix, Host IP, API key), the
`applications` table (one row per Application, with its Compose definition in
the `compose` column), `application_env` and `api_keys`. Read it to re-create
your Applications; then:

```bash
self-host reset   # removes the containers, volumes and configuration directory
self-host init
```

Application IDs and the CA do not survive this. Consumers have to trust the new
CA, and containers are recreated under new names. Docker named volumes are
removed by `reset` — copy anything you need out of them first.

## Upgrading from a Platform that ran Traefik

Platforms up to 0.2.0-beta.2 served HTTP and HTTPS from a `sf-system-proxy`
container. This version serves them itself, and takes the ports on start:
the container is removed, and so are the Compose file that would hand them
back on the next `docker compose up` and the Traefik configuration next to it.
Nothing there is state — all of it was generated from records the Platform
still has.

Applications deployed before the upgrade have no Host port for the Platform to
reach them on, and a running container cannot be given one. Each is recreated
once, on the first start after upgrading, and answers again when it comes back:

```
Application hermes answers on Host port 51268 now; its workload was recreated
to publish it
```

Volumes, bind-mounted data, Hostnames, aliases and Application IDs all survive
that — it is a recreate of the workload, not a redeploy. A Host whose Docker
cannot be reached migrates nothing, says so, and answers those Applications
with a 503 until Docker is back and the daemon restarts.

The `sf-system` bridge is left behind with no members. The daemon removes it
on its next start, on the same pass that takes the ports back from the
`sf-system-proxy` container; on a Host whose Docker cannot be reached it
stays until the daemon is back.

## Failure and recovery

A mutation is one file replaced atomically, so an interrupted write leaves the
previously committed record in place. On start the daemon collects what an
interrupted write left behind: a Compose file no record references, and an
Application directory that never got a record.

A deploy interrupted by a restart is settled against what Docker is actually
running: running means running, anything else is reported as failed with the
reason. When Docker itself cannot be reached the deploy stays pending and stays
visible, because an executor that answers nothing has observed nothing. A
Docker outage never turns an Application into a stopped one.

State that does not parse, or that a newer Platform wrote, stops the daemon
with the file's path and the reason. It is never read as an empty installation.
Restore from a backup, or `self-host reset` and start over.

Durability is what the filesystem gives you. Each write is flushed, renamed
over its destination, and the directory flushed after it; on macOS the flush is
`F_FULLFSYNC`, which is what actually reaches the drive there. On a filesystem
that will not flush a directory, the rename is still atomic for a reader but
its survival across a power cut is not guaranteed. Network filesystems are not
supported for `state/`.

## Running without Docker

DNS, the Operator API, the console and everything in `state/` work on a Host
where Docker is missing, stopped or broken. You can read your configuration,
see your Applications and change settings that do not need a workload.

Docker is required to run Applications, and — until the proxy moves into the
binary — to serve public HTTPS. Operations that need it fail with what went
wrong rather than hanging, and nothing you saved is lost.
