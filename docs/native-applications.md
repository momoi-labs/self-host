# Native applications

A Native Application is a process tree the Platform runs on the Host itself,
without a container, under an account that exists only for it. This
page covers account provisioning, privilege dropping, cgroups and s6
supervision. The console and Operator API create and operate these Applications through
its existing task queue. Install the protected binary and start
`self-host-native.service` before accepting native requests.

The sections below describe the Linux runtime. The macOS MVP uses the same
Application model with a restricted helper and foreground process groups.
See [macOS native setup and validation](macos-native-applications.md) for its
installation, supported operations and release gates.

## The Application Account

Every Native Application gets one Linux system account, named
`sf-app-<id>` after the Application's id, the same prefix the container
Runtime uses. The account is an execution identity, not a login:

- its login shell is `/usr/sbin/nologin`; the authenticated console terminal
  starts an explicit shell under the same account;
- it has a group of its own and no supplementary groups, ever, so it is in
  neither `sudo` nor `docker`;
- its home is mode `0750` and owned by the account, so another Application
  Account cannot read it;
- its comment field carries `self-host Application <id>`, which is how the
  Platform recognises an account it created.

`native::identity::AccountName` checks the name (1 to 32 characters,
lowercase letters, digits, `_` and `-`, starting with a letter or `_`, never
`root`). `resolve` looks it up and refuses uid 0, gid 0 and the uid the
Platform itself runs as, so a misconfigured record can never point an
Application at root or at the Platform's own files.

## Application environment

The native form reuses the custom image dependency editor. Select mise tools
and versions, add mise tool options such as `extras=serve,ane` on a pypi tool
or `allow_builds` on an npm tool, then add optional setup commands. Installation and setup run under the dedicated Application Account
and its cgroup limits before the main command starts. The console opens Logs
for preparation output. A failed installation fails the Task and prevents
startup.

Each Application keeps its mise executable, configuration, tools and caches
under `~/.self-host/mise/`. The Platform supplies private mise and XDG paths;
it does not inherit the Operator's tools or credentials. Mise documents these
[configuration paths](https://mise.jdx.dev/configuration.html#environment-variables)
and its [user installation path](https://mise.jdx.dev/installing-mise.html).
An empty recipe needs no network bootstrap, so existing native definitions
continue to work without downloading tools.

```json
"recipe": {
  "dependencies": [{"tool": "node", "version": "24"}],
  "setup": ["mkdir -p app", "node --version"]
}
```

The main command and authenticated terminal use the same installed tools.
For example, the program can be `node` without an absolute path. Automatic
installation is disabled while running the Application or opening a terminal.
Setup commands also run without root. Host package manager operations that
need administrative privileges cannot run from this field.

The Platform records successful recipe application separately from Variables.
Ordinary Start, Restart, crash recovery and Variable edits do not repeat setup.
Changing the recipe applies it again, including while the Application is
stopped, and preserves that stopped intent. Commands should tolerate repeated
execution after an interrupted or failed preparation. Tool installation and
setup are not transactional and do not restore previous files on failure.

Preparation has a 15-minute limit and bounded output. Application Variable
values are redacted before the log is stored. Stopping preparation cleans its
process tree through the same cgroup boundary as other native commands.

## Provisioning is privileged, launching is not

`provision` runs as root. It calls `useradd --system --user-group
--create-home --shell /usr/sbin/nologin` and then sets the home to `0750`,
owned by the account. Running it twice is a no-op. An account by the same
name that the Platform did not create is refused (`NotOurs`), never adopted.

`launch` needs root too, to move the child into its cgroup and switch to the
account, but the command itself never runs with any of that privilege. The
parent checks everything before it forks:

1. the account resolves and is not root or the Platform;
2. the working directory is absolute, spelled without `..`, and is the
   account's home or a path under it;
3. the command is not empty and the environment does not set `HOME`, `USER`
   or `LOGNAME`;
4. the cgroup exists with its limits written.

A refusal spawns nothing and leaves no cgroup behind. Then the child, between
`fork` and `exec`, does this in order and fails closed at every step. A
failure makes the child exit without running the command and `launch`
reports it. There is no fallback and no run as root to "fix later".

1. Write its own pid into the cgroup's `cgroup.procs`, while still
   privileged.
2. `prctl(PR_SET_NO_NEW_PRIVS, 1)`: nothing the process execs can gain
   privilege, so setuid binaries such as `sudo` do nothing for it.
3. `prctl(PR_CAPBSET_DROP)` for every capability the kernel knows. This
   happens before the uid changes because it needs `CAP_SETPCAP`, which the
   uid change takes away.
4. `setgroups(0, NULL)`: no supplementary groups.
5. `setresgid` and then `setresuid` to the account, all three ids each.
6. Read back `getresuid` and `getresgid` and fail if any of the six is 0 or
   not the account's.
7. `prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_CLEAR_ALL)`.
8. `umask(0o027)`.
9. Change to the requested working directory under that account.

The command starts with `env_clear()`: only `HOME`, `USER`, `LOGNAME`,
`PATH=/usr/local/bin:/usr/bin:/bin` and whatever the request adds. The
daemon's environment does not leak, so an `SSH_AUTH_SOCK`, a `DOCKER_HOST` or
the Platform's own API key set on the daemon never reaches an Application.
The main process has `/dev/null` as stdin and pipes for stdout and stderr.
A console terminal uses a pseudo-terminal for all three streams.

Every launch has a `Purpose` (`Main`, `Hook`, `Build`, `Terminal`) and all
of them go through the same steps. Readiness commands use `Hook` and join the
main command's existing cgroup through the same N1 launch checks. A build
or terminal has the Application's permissions.

## What the cgroup does

The Platform owns one cgroup v2 directory and creates
`<root>/sf-app-<id>` under it for each Application, with `cpu`, `memory`
and `pids` enabled. The whole process tree lives there, detached
grandchildren included, and the directory is where `ResourceLimits` land:

| Limit | File | Written as |
| --- | --- | --- |
| `cpu_percent: 50` | `cpu.max` | `50000 100000` (a quota over a 100 ms period; 100 is one CPU) |
| `memory_bytes: 33554432` | `memory.max` | `33554432` |
| `max_tasks: 8` | `pids.max` | `8` |
| left unsaid | any of them | `max` |

`memory.oom.group` is set to `1`, so when the tree goes past its memory the
kernel kills all of it, not one process it picked.

`stop` sends `SIGTERM` to the leader, waits a grace period, then writes
`cgroup.kill`, which ends every process still in the group at once. It
waits for `cgroup.procs` to empty and removes the directory. A process that
called `setsid` to get away from its parent does not get away from this.

## What the cgroup does not do

A cgroup is a resource control, not a sandbox. It does not hide the
filesystem, the network or other processes. Ownership and modes do the
filesystem part: the Application sees `/etc/passwd` like any account and
cannot read another account's `0750` home or the Platform's `0600`
credentials in a `0700` directory. The network part is not addressed in
this slice.

## What the tests prove

`tests/native_linux.rs` runs as root on a disposable Linux Host (every test
is `#[ignore]`, so an ordinary `cargo test` skips them). Each test creates
its own cgroup root under `/sys/fs/cgroup` and its own accounts, and removes
both when it ends, also on failure.

1. Provisioning: a system account with a `nologin` shell, a `0750` home it
   owns, no supplementary groups, not in `sudo` or `docker`; a second
   Application cannot read the first one's home; provisioning twice is a
   no-op and someone else's account is refused.
2. Identity in the launched process: `id -u` and `id -g` are the account's
   and non-zero; `CapEff`, `CapPrm`, `CapBnd` and `CapAmb` are all zero;
   `NoNewPrivs` is 1; `Groups` is empty; `HOME` is the account's and none
   of the daemon's variables leaked.
3. Refusals with nothing spawned: an empty name, `root`, an unknown
   account, an empty command, a working directory outside the home, a
   `HOME` override, a cgroup root that is not cgroup2.
4. Descendants: a detached `setsid sleep` grandchild is in the cgroup and
   dead after `stop`, and the cgroup directory is gone.
5. `max_tasks: 8` against a loop forking 30 sleeps: never more than 8 pids
   and `pids.events` counts the refused forks.
6. `memory_bytes: 32 MiB` against a `python3` that touches 128 MiB: the
   process dies by `SIGKILL` and `memory.events` counts an `oom_kill`.
7. `cpu_percent: 50` reads back from `cpu.max` as `50000 100000`.
8. A root-owned `0600` file in a `0700` directory: `cat` as the account is
   refused.
9. The account is in neither `docker` nor `sudo`; the Docker socket, when
   there is one, is not writable by it; `sudo -n true` fails.
10. Every `Purpose` passes the same identity checks as test 2.

## Operator API

Create a worker through `POST /apps`. Omit `account` or send an empty
string. The Platform assigns `sf-app-<id>` and provisions that dedicated
non-root account. A supplied account must match the assigned identity.

```json
{
  "name": "synthetic-worker",
  "runtime": {
    "kind": "native",
    "command": ["/bin/sleep", "300"],
    "limits": {"cpu_percent": 50, "memory_bytes": 134217728, "max_tasks": 32}
  },
  "publication": {"kind": "unpublished"},
  "environment": {"SYNTHETIC": "example"}
}
```

The account home is `/var/lib/self-host/native-data/<id>`. The root-owned
parent is traversable but does not list its contents. Each home is `0750`
and belongs to its account. `working_dir` is relative to that home; omit it
to run in the home itself. Setup runs from the home and can create the
directory. After setup, the directory must exist and resolve inside the home.
Absolute paths, `.` and `..` are refused before recording a request.
The private supervision directory stays separate and inaccessible to
Application Accounts.

A Web Target sets `publication.kind` to `web` and a non-reserved port of at
least 1024 in `runtime.port`. Its command must bind that port on
`127.0.0.1` under its Application Account. The Platform checks Linux's socket
inventory and waits for the loopback listener before publishing a route.
A listener on `0.0.0.0`, `::`, another address or another account fails the
task and stops the Application. A worker has no port, Hostname or route.
The Platform refuses ports already assigned to another Application, managed
PostgreSQL transport ports, and its own API and proxy ports. Managed database
port reservations remain active while the database is stopped.

Create and definition updates answer `202` with the existing task id. Use
`POST /apps/id/<id>/start`, `/stop` and `/restart` for lifecycle actions.
`PUT /apps/id/<id>` accepts the native Runtime, name, Hostname, aliases
and route rules. Runtime kind and account cannot change. Updating a stopped
Application replaces its definition and keeps it stopped. Name and route
edits finish inline without restarting the process.

`environment` on creation reaches the first launch. Later Variable changes
use the existing `/apps/<name>/env` routes and task queue. Identity Variables,
`MISE_*`, the four XDG home paths, `CARGO_HOME` and `RUSTUP_HOME` are reserved.
A stopped Application stays stopped when its Variables change.

The existing detail and log API routes report s6 observation and stream
preparation output and the Application's private s6 logs. An unavailable
supervisor reports an unknown service state with its cause. The console shows native configuration,
account identity, readiness, logs, terminals and cgroup resource usage.

Removal stops the process tree and removes its generated supervision files.
The Platform verifies account ownership, revokes the account and makes its
retained home root-owned `0700`. It keeps the data and log files. Reusing a
Linux uid cannot expose that retained home. Removal does not delete data.

## Console operation

Choose **New application**, then **Native process**. Enter a single **Start
command**, such as `node server.js --port 3000`. Quotes keep spaces and empty
arguments intact. The Platform passes argv directly; use `sh -c` when shell
syntax is needed. Choose dependencies and versions in
**Mise packages and dependencies**, then add optional **Custom commands**.
The working directory is relative to the private Application home and must
exist after setup. An empty working directory uses the home.

Set CPU, memory and task limits in **Resource limits**. Under **Publication**,
choose **Hostname** or **No hostname**. Published Applications also need a
loopback port. Publication is fixed at creation and appears as a fact when
editing. The Summary tab shows the account, program, readiness and
limits. Configuration, Start, Stop, Restart and Remove use the existing task
queue. Restart does not offer container image options.

Application Variables can be supplied before the first launch. The Variables step
in Configuration loads only names through `GET /apps/id/<id>/variable-names`; it never
fetches saved values. Add and Replace open a dialog with a masked input. Remove
uses the existing Variable API. The explicit `/apps/<name>/env` API remains
available for a deliberate authenticated value reveal. Do not put credentials
in command arguments, which are ordinary Application configuration.

## Native terminals and resource usage

The Terminal tab authenticates its first WebSocket frame with the Operator
API key. The key is not part of the URL. Only a running Native Application can
open a terminal. An unavailable supervisor or unsupported Host refuses the
request. A native request cannot select a container.

The terminal starts `/bin/sh -i` through the same privilege-drop boundary as
the main process. It has the Application Account, home, Variables and
cgroup, with `TERM=xterm-256color`. It does not inherit the daemon's
credentials or descriptors. Opening or closing it never recreates the
Application cgroup, changes its limits or restarts the main process.

Disconnect closes the PTY and signals processes carrying that terminal's
random session marker, including detached children that keep the marker.
Signals use pidfds to avoid a recycled PID. A process that deliberately
removes the marker may outlive its terminal; it still runs under the same
account and cgroup. Stopping the Application ends the entire cgroup.

Glance and Overview read native CPU, memory and task counts from cgroup v2.
CPU is a delta across ticks; the first sample starts at zero. The count
includes terminal processes and threads. Stopped Applications leave a gap,
and a recreated cgroup starts a new CPU baseline. Native cgroups do not
supply network byte counters, so their Glance shows tasks instead. The
Overview's network totals cover containers and Virtual machines only.

Before output reaches s6-log, the native runner combines stdout and stderr
and replaces literal nonempty Application Variable values with `[redacted]`.
The matcher covers values split across reads or the two pipes, and flushes
the final prefix only when the combined stream closes. It buffers at most a
possible value prefix plus the current read. A bounded queue holds up to 16
reads of 8 KiB each. It uses only explicit Application Variables, never default
identity or runtime values. Each launch retains its own redaction values, so
later Variable rotation does not expose its old values in captured logs.

This is literal matching. Encoded, transformed or partial secrets and
interactive terminal output are outside it. Logs captured by earlier versions
remain unchanged on disk. The log API also masks current Variable values
when reading them. Terminal output is deliberate authenticated Operator
access and is not copied into Application logs.

## Interrupted tasks and daemon restart

The daemon reconnects to the running s6 tree. It does not start another
scanner or duplicate an Application process. Tasks that were running when
the daemon stopped fail in the audit history. The Application record is
reconciled against s6's persistent `down` intent and ready state. A task that
never started remains queued. If the supervisor is unavailable, an
interrupted pending record stays pending and has no route.

A definition replacement first stops the old tree and writes the new private
definition while the `down` file remains present. A daemon failure before
start therefore leaves the Application stopped. A later explicit start runs
the recorded definition only if preparation succeeded for its recipe. Reapply
the configuration to retry failed preparation. Failed startup withdraws the
route and records the failure on the Application and task.

## Supervision and intended state

`native::supervision::Supervisor` connects to a running s6 scan tree. It does
not start a second scanner when the daemon restarts. `prepare` creates a
stopped service; `start`, `stop` and `restart` change it and wait for s6's
ready or fully stopped notification. A failed readiness check reports a
startup error with the Application's log directory.

The root-owned `down` file records stopped intent. Starting removes it and
stopping writes it before sending a control command. Both operations sync
the directory. s6 reads the same file after reboot. The service definition
is private JSON, separate from Application data, and includes argv,
Variables, limits, readiness and timeouts. The lifecycle adapter maps
Platform State to this internal representation. The Operator API accepts
the public NativeDefinition shape.

Each generated `run` script invokes the internal `native-run` command. That
runner calls N1, forwards stdout and stderr, runs readiness under the same
Application Account, then notifies s6. Main commands and readiness commands
inherit no supervisor descriptors or daemon credentials. The runner handles
stop with N1's grace period and `cgroup.kill`. The `finish` script repeats
cgroup cleanup when a runner crashes or is killed. The `s6-setlock` lock prevents
a replacement from overlapping a still-running wrapper.

All supervisor paths must be absolute, root-owned, free of symlinks and
unwritable by other accounts, including their ancestors. Service definitions
and wrappers are inaccessible to Application Accounts. Keep the executable
in a protected directory too. A writable development checkout is refused.

s6-log captures each Application in its own private log directory, owned by
the Platform. It rotates at 1 MiB and keeps ten old files. The Application
cannot edit its log history or another Application's logs. Application data
stays in the Application Account's home and is never removed by stop or
restart.

## Linux boot supervision

Linux releases include the s6 supervision tools. The installer and Linux
packages place them in `/usr/libexec/self-host/s6`; they do not replace a
distribution's s6 installation. Linux must expose cgroup v2 with `cpu`,
`memory`, `pids` and `cgroup.kill`.

`assets/linux/self-host-native.service` starts one root-owned s6 tree with
`Delegate=cpu memory pids`. The boot command moves the supervisor into a
separate cgroup leaf before enabling controllers for Application cgroups.
The service manager owns the scanner's lifetime; s6 owns Application
processes. Restarting the Platform daemon does not restart this unit.

The Linux installer opts into that unit with `SELF_HOST_NATIVE=1`. It checks
the installed tools and cgroup v2 before enabling it. The default installer
installs the bundle without enabling the unit. Installing the unit does not
create any Applications. Native requests require the privileged Linux daemon
and protected installed binary.

Source builds can install the distribution's `s6` package, such as
`sudo apt-get install s6` on Debian or Ubuntu. The runtime uses those tools
only when no private release bundle is installed. An incomplete private
bundle fails instead of mixing versions. Reinstall the release to repair it.

For development on a disposable Linux Host, install the built binary and
unit with root ownership, then start the unit:

```sh
sudo install -o root -g root -m 755 target/debug/self-host /usr/local/bin/self-host
sudo install -o root -g root -m 644 assets/linux/self-host-native.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now self-host-native.service
sudo systemctl status self-host-native.service
```

The scan directory is `/var/lib/self-host/native/services`. The boot command
writes the delegated Application cgroup path to
`/var/lib/self-host/native/cgroup-root`. `Supervisor::connect` takes that
path and the installed executable. Missing s6, unsafe supervision paths and
missing controllers fail before Application code runs. Use
`journalctl -u self-host-native.service` for boot failures.

## API lifecycle validation

`tests/native_api_linux.rs` uses fresh root-owned fixtures, real s6 and
cgroups, a file-backed state store and the HTTP Application API. It checks
task outcomes, non-root identity, zero capabilities, limits, logs, stopped
updates, Variable changes, daemon reconnect, private Web Targets, worker
publication, unsafe binding refusal, account revocation and retained data.
The terminal check also covers first-frame authentication, non-root identity,
capabilities, the cleared environment, PTY sizing, a detached child's cgroup,
resource sampling and disconnect cleanup that preserves the main process and
its cgroup. It verifies variable-name responses and stdout/stderr redaction
in the stored s6 log. This check requires `python3` to create the detached
fixture child. Docker is a fake boundary in this native-only fixture. The ordinary
container suite still validates its lifecycle separately.

Run these tests only on a disposable Linux Host:

```sh
cargo test --test native_api_linux -- --ignored --test-threads=1
```

A skipped ignored test is not Linux validation evidence.

For real mise downloads, run `scripts/test-native-mise.py` as root against an
isolated loopback daemon with `SELF_HOST_TEST_URL` and `SELF_HOST_TEST_KEY` set.
It installs Node 22 and 24 under different Application Accounts and checks
setup, private paths, stopped updates, failure recovery and redacted logs. It
removes only its own fixtures after success and retains them on failure.

## Supervision validation

`tests/native_supervision_linux.rs` runs only with explicit `--ignored` on a
disposable root Linux environment. It checks main and readiness identities,
zero capabilities, protected wrappers, crash recovery, detached descendant
cleanup, log rotation, reconnect without duplication, stopped intent after
scanner restart, retained data and unhealthy-startup errors. These checks
complement the N1 privileged tests; skipped tests are not Linux evidence.

The s6 behavior follows its [service directory
contract](https://skarnet.org/software/s6/servicedir.html) and
[control commands](https://skarnet.org/software/s6/s6-svc.html).

For a real reboot check, use the fixture only on a disposable Host after
installing the unit above. Build and prepare the two synthetic Applications:

```sh
cargo build --example native_reboot
sudo target/debug/examples/native_reboot prepare
```

Reboot that disposable Host, wait for `self-host-native.service` to start,
then verify:

```sh
sudo target/debug/examples/native_reboot verify
```

The fixture checks that the boot id changed, the running Application started
exactly twice, the stopped Application started once, both retained their
data, and the running process uses a nonzero uid. It leaves the fixtures in
place for inspection. Destroy the disposable Host when validation ends.

## Fresh API reboot fixtures

For N3, use `examples/native_api_reboot.rs` against the running disposable
daemon and installed native service. Set `SELF_HOST_TEST_API` to that daemon's
Operator API and `SELF_HOST_TEST_API_KEY` to its synthetic test key. The
fixture manifest contains no credential. Use a new manifest for each check:

```sh
cargo build --example native_api_reboot
sudo -E target/debug/examples/native_api_reboot prepare /var/lib/native-api-reboot-fresh.json
```

Reboot the disposable Host, start its isolated daemon with the same state,
and run `verify` with the manifest. The check requires a changed boot id,
exactly two starts for the running Application and one for the stopped
Application, non-root identity, zero capabilities, and retained data:

```sh
sudo -E target/debug/examples/native_api_reboot verify /var/lib/native-api-reboot-fresh.json
```

Do not reuse old manifests or accumulated start counters. Destroy the
disposable Host after retaining sanitized validation results.
