# Native Applications

A Native Application is a process tree the Platform runs on the Host itself,
without a container, under a Linux account that exists only for it. This
page covers the part that is built: the account, the privilege drop and the
cgroup. Nothing here is reachable from the Operator API yet. The record can
say `"runtime": {"kind": "native"}` (ADR-0028), but create and update refuse
it until the lifecycle slice lands. s6 supervision (ADR-0014) is also later.

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
of them go through the same steps. A build or a terminal is not a way to get
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

- Supervision with s6 (ADR-0014): restart on exit, logs, readiness.
- The Operator API: creating, deploying and stopping a Native Application
  through the console. Until then the record shape exists and is refused.
- Turning a `NativeDefinition` plus the Application's Variables into a
  `LaunchRequest`, described as a proposed shape in
  `docs/deployment-contracts.md`.
