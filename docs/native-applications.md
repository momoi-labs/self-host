# Native Applications

A Native Application is a process tree the Platform runs on the Host itself,
without a container, under a Linux account that exists only for it. This
page covers account provisioning, privilege dropping, cgroups and s6
supervision. The Operator API still refuses native create and update requests
until N3 connects the lifecycle adapter. The internal supervisor does not
change that refusal.

Linux only. On any other system the same functions return an `Unsupported`
error and nothing else happens.

## The Application Account

Every Native Application gets one Linux system account, named
`sf-app-<id>` after the Application's id, the same prefix the container
Runtime uses. The account is an execution identity, not a login:

- its shell is `/usr/sbin/nologin`, so nobody opens a session as it;
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

The command starts with `env_clear()`: only `HOME`, `USER`, `LOGNAME`,
`PATH=/usr/local/bin:/usr/bin:/bin` and whatever the request adds. The
daemon's environment does not leak, so an `SSH_AUTH_SOCK`, a `DOCKER_HOST` or
the Platform's own API key set on the daemon never reaches an Application.
stdin is `/dev/null`; stdout and stderr are pipes the caller reads.

Every launch has a `Purpose` (`Main`, `Hook`, `Build`, `Terminal`) and all
of them go through the same steps. Readiness commands use `Hook` and join the
main command's existing cgroup through the same N1 launch checks. A build or a terminal is not a way to get
more than the Application has.

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

## What comes later

- The Operator API: creating, deploying and stopping a Native Application
  through the console. Until then the record shape exists and is refused.
- Turning a `NativeDefinition` plus the Application's Variables into a
  `LaunchRequest`, described as a proposed shape in
  `docs/deployment-contracts.md`.

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
Variables, limits, readiness and timeouts. N3 will map Platform State to this
internal representation; the Operator API does not accept it yet.

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

Install the distribution's `s6` package. Linux must expose cgroup v2 with
`cpu`, `memory`, `pids` and `cgroup.kill`. On Debian or Ubuntu:

```sh
sudo apt-get install s6
```

`assets/linux/self-host-native.service` starts one root-owned s6 tree with
`Delegate=cpu memory pids`. The boot command moves the supervisor into a
separate cgroup leaf before enabling controllers for Application cgroups.
The service manager owns the scanner's lifetime; s6 owns Application
processes. Restarting the Platform daemon does not restart this unit.

The Linux installer opts into that unit with `SELF_HOST_NATIVE=1`. It checks
for s6 and cgroup v2 before installing and enabling it. The default installer
keeps its existing container-only behavior. Installing the unit does not
enable native API requests or create any Applications.

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
