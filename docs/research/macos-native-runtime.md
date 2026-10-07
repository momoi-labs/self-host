# macOS native runtime MVP

Date: 2026-10-03. Research was read-only with respect to Host accounts and
services. Implementation followed in the same branch; see the validation
status in [the macOS native guide](../macos-native-applications.md).

The proposed MVP runs each Native Application under its own non-admin macOS
account. Linux already uses this identity model; port its account and launch
operations to Darwin. Keep the Platform API under the Operator and reuse s6
through a narrow privileged helper. Linux behavior remains unchanged. The
first implementation gate is a failing s6 shutdown smoke test described
below.

## Proposed scope

Reuse one s6 scan tree, its service definitions, readiness notifications,
logs, and persisted running or stopped intent. Protect the macOS scan tree
and helper with root ownership. Give each Application a separate private
data home. This is a proposal, not implemented behavior.

Install one independent root LaunchDaemon for the scan tree. Its launcher
drops privileges before executing Application commands. Keep the existing
Platform LaunchDaemon under the Operator so Docker, Git, and configuration
retain their current identity and paths. System daemons start at boot; user
LaunchAgents are tied to login and terminate at logout. Boot behavior still
needs a real Host test. [Apple launchd guide](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html),
[Apple launchd.plist manual](https://github.com/apple-oss-distributions/launchd/blob/main/man/launchd.plist.5).

Support foreground commands, lifecycle actions, Variables, logs, and a
loopback Web Target. Defer terminal access, resource metrics and limits,
Git source deployment, and managed PostgreSQL connections. The last item
already requires a dedicated `sf-app-*` identity in
[`NativeEndpoint::new`](../../src/connectivity.rs), but macOS connectivity
still needs separate end-to-end validation.

## Account and privilege boundary

Provision accounts through a fixed root-owned `self-host native-control`
entry point invoked with `/usr/bin/sudo -n <protected-bin> native-control`.
The installer grants only this exact command and argument to the configured
Operator, validates the sudoers file, and removes the grant on uninstall.
Protect the binary and every parent directory from unprivileged writes.
No privileged API server or generic root command runner is needed.

The helper accepts bounded typed JSON on stdin. It validates the operation,
Application ID, ownership marker, and fixed native data roots independently
of the API. It clears inherited environment variables and never evaluates
request data as a root shell command. A sudoers rule without specified
arguments permits arbitrary arguments; the fixed subcommand is required.
`sudo -n` fails if authorization requires a password instead of prompting.
[Sudoers command matching](https://github.com/sudo-project/sudo/blob/main/docs/sudoers.man.in),
[Sudo noninteractive execution](https://github.com/sudo-project/sudo/blob/main/docs/sudo.man.in).

Proposed Darwin account provisioning uses `/usr/bin/dscl` against the local
directory. Serialize UID/GID allocation with a Platform lock and check for
collisions before accepting the result. Store and verify an ownership
marker, create a dedicated group, disable authentication, use a refusing
login shell, and give the account no admin or supplementary groups. Set
its home to mode `0750`. Never adopt or delete an account without verifying
the marker, IDs, group, and expected home. `IsHidden` can hide the account;
it does not disable authentication. This provisioning sequence remains
untested. [Apple account visibility](https://support.apple.com/en-us/102099).

For a privileged child, Darwin `setgid` and `setuid` set real, effective,
and saved IDs. `setgroups` also opts out of external group expansion through
memberd. Use `setgroups(1, [app_gid])`, then `setgid(app_gid)` and
`setuid(app_uid)`, checking every return value. A zero-length Darwin group
list is internally represented by group 0, so the explicit single group
avoids relying on Linux's empty-list semantics. Verify IDs, saved IDs, and
the remaining group list before exec; Darwin process information exposes
the saved IDs. Any failure must prevent Application execution. These API
semantics were checked in Apple documentation and source; the complete
launcher still needs tests. [Apple setuid manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/setuid.2.html),
[XNU identity implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_prot.c),
[Apple process information structures](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/proc_info.h).

This research found no documented direct equivalent of Linux
`no_new_privs`. Dropping UID/GID does not establish the same restrictions on
later set-ID executable launches. [Apple process launch semantics](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/posix_spawn.2.html).
Record the macOS limits explicitly in
[ADR-0031](../adr/0031-application-accounts-are-execution-identities-and-cgroups-are-resource-controls.md)
and validate the launch boundary before release. Account separation alone
does not provide cgroup guarantees or a filesystem sandbox.

## Supervision and stopping

s6 requires a POSIX system, C compiler, GNU make, and skalibs. Its upstream
documentation covers macOS, and the repository already pins s6 and skalibs
2.15.1.0. Reusing s6 avoids a second per-Application service manager.
[s6 requirements](https://skarnet.org/software/s6/),
[pinned release script](../../scripts/release-s6.sh).

Verified mechanisms in the pinned s6 source:

- `s6-supervise` starts `run` in a new session.
- `timeout-kill` plus `flag-timeout-killpg` escalates a stop timeout to the
  service's process group.
- `finish` receives the previous run's PGID as its fourth argument. It can
  clean up same-group children after the leader exits.
- `s6-svc -K` signals the process group. There is no `-G` option.

These mechanisms are documented by
[s6-supervise](https://skarnet.org/software/s6/s6-supervise.html),
[service directories](https://skarnet.org/software/s6/servicedir.html), and
[s6-svc](https://skarnet.org/software/s6/s6-svc.html). The two service-directory
mechanisms were also checked in the downloaded 2.15.1.0 source.

Process groups do not match Linux whole-tree teardown. `setsid()` creates a
new session and process group; descendants that detach can escape group
signals. Per-Application launchd does not fix this because its documented
cleanup also targets the original group. Require foreground execution and
state that detached descendants are unsupported. UID-wide teardown is out
of this MVP. [Apple setsid manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/setsid.2.html),
[Apple launchd.plist manual](https://github.com/apple-oss-distributions/launchd/blob/main/man/launchd.plist.5).

For Web Target readiness, a successful TCP connection or matching UID is
insufficient. The MVP should inspect listening sockets and verify both
loopback binding and the Application Account, with its active PGID as the
foreground service boundary.
`/usr/sbin/lsof` has process-group selection and machine-readable output;
use bounded execution and parse fields, not the formatted table. This
replacement for `/proc/net/tcp` needs fixture tests for wildcard listeners
and another Application's listener. [lsof options](https://github.com/lsof-org/lsof/blob/master/docs/options.md).

## Resource controls

The API and helper must reject nonempty `cpu_percent`, `memory_bytes`, and
`max_tasks` on macOS instead of silently ignoring them. `RLIMIT_CPU` counts
CPU seconds per process, not an Application's CPU percentage.
`RLIMIT_NPROC` counts processes for a UID, not the cgroup task count that
includes threads. A per-process address-space limit is not an aggregate
Application memory ceiling.
[Apple setrlimit manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/setrlimit.2.html),
[XNU resource implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_resource.c).

Return unavailable metrics for this MVP. Darwin's `libproc` can later supply
per-process CPU, resident memory, and thread counts, but summing sampled
processes needs explicit treatment of exited children, PID reuse, shared
memory, and processes that change groups. It is not a cgroup counter.
[Apple libproc header](https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/wrappers/libproc/libproc.h),
[Apple process information structures](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/proc_info.h).

## Local s6 build and shutdown failure

This research compiled the repository's pinned, checksum-verified upstream
tarballs in a temporary directory. Environment:

| Item | Tested value |
| --- | --- |
| Host | Apple Silicon, macOS 26.6.2, build 25G83 |
| Compiler | Apple clang 21.0.0, clang-2100.1.1.101 |
| Build tool | GNU Make 3.81 |
| Packages | s6 2.15.1.0, skalibs 2.15.1.0 |
| Compiler flags | `CC=clang`, `CFLAGS=-Os` |
| Result | arm64 Mach-O tools, linked to system `libSystem.B.dylib` |

The skalibs configure command was:

```sh
./configure --prefix="$work/deps" --disable-shared
```

The s6 configure command was:

```sh
./configure --disable-execline --enable-absolute-paths \
  --bindir="$work/bundle/bin" --libexecdir="$work/bundle/bin" \
  --with-sysdeps="$work/deps/lib/skalibs/sysdeps" \
  --with-include="$work/deps/include" --with-lib="$work/deps/lib"
```

Both used `make -j2`; skalibs also used `make install` into the temporary
prefix. The ten tools from `scripts/release-s6.sh` were built. The build
did not use the Linux script's musl target, `/proc` overrides, or
`--enable-static-libc`. Upstream supports static skarnet libraries while
retaining the system C library. [s6 installation instructions](https://github.com/skarnet/s6/blob/v2.15.1.0/INSTALL).

Confirmed: compilation, unprivileged scan startup, service startup,
`s6-svstat`, and persisted service stop through `s6-svc -D` plus
`s6-svwait -D` succeeded. The fixture was a `run` script containing
`exec /bin/sleep 60`.

Failed: stopping the scanner with `s6-svscanctl -t` reproducibly printed:

```text
s6-svscan: warning: unable to check internal pipes: Input/output error
s6-svscan: warning: executing into .s6-svscan/crash
s6-svscan: fatal: unable to exec .s6-svscan/crash: No such file or directory
```

It exited 127 because the fixture had no crash handler. The preceding
internal-pipe failure is the issue; adding a crash handler would not fix it.
Fixture supervisors were explicitly stopped afterward. No research fixture
processes remained at handoff.

Minimal reproduction after building the tools, with `bin` set to the
temporary bundle directory:

```sh
scan=$(mktemp -d)
mkdir "$scan/app"
printf '#!/bin/sh\nexec /bin/sleep 60\n' > "$scan/app/run"
chmod 755 "$scan/app/run"
"$bin/s6-svscan" "$scan" > "$scan/stdout.log" 2> "$scan/stderr.log" &
scanner=$!
sleep 1
"$bin/s6-svwait" -u -t 5000 "$scan/app"
"$bin/s6-svc" -D "$scan/app"
"$bin/s6-svwait" -D -t 5000 "$scan/app"
"$bin/s6-svscanctl" -t "$scan"
wait "$scanner"
cat "$scan/stderr.log"
"$bin/s6-svc" -dx "$scan/app"
```

The likely cause is the pinned skalibs select backend. On Apple it selects
`iopause_select`; `IOPAUSE_READ` includes `POLLHUP`, and the select adapter
copies that mask into the returned events for a readable descriptor.
s6-svscan then treats `POLLHUP` as an internal-pipe exception.
[backend selection](https://github.com/skarnet/skalibs/blob/v2.15.1.0/src/libstddjb/iopause.c),
[event constants](https://github.com/skarnet/skalibs/blob/v2.15.1.0/src/include/skalibs/iopause.h),
[select adapter](https://github.com/skarnet/skalibs/blob/v2.15.1.0/src/libstddjb/iopause_select.c),
[scanner event loop](https://github.com/skarnet/s6/blob/v2.15.1.0/src/supervision/s6-svscan.c).

A separate local probe confirmed that an ordinary readable pipe produced
`revents=0x11` through `iopause_select`, including `POLLHUP=0x10`; plain
`poll()` on the same pipe returned `0x1`. The implementation now applies a
macOS-only patch that removes exception bits from select readability.
The smoke test passes startup, crash recovery,
foreground child cleanup, persistent stop and scanner shutdown.

Temporary source, binaries, build log, and event probe were retained at
`/var/folders/_3/kvhr6qyn3k7fcg5n4prqcp700000gn/T/self-host-macos-s6-research-5zr6o5mz`.
The research phase changed no runtime code. `bin/validate-prose` is absent
in this repository, so the note received a manual prose review and punctuation
check.

The privileged account/lifecycle fixture and boot persistence are still release
gates. Start acceptance with Apple Silicon. Intel builds and the full native
workflow still need their own validation.
