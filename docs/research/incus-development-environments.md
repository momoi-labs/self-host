# Incus development environments

Research and smoke test performed on September 11, 2026 against the home
server's Apple Silicon Mac. The test used a new Colima profile named
`self-host-incus-test`; the existing `default` and `self-host-test` profiles
were left running and unchanged.

## Result

Incus is suitable for the native development environment path when the guest
only needs Linux processes, systemd, SSH and a persistent filesystem. An
Ubuntu 24.04 system container booted with `/sbin/init` as PID 1 and reported
`systemctl is-system-running=running`. Git, build tools, Python, OpenSSH and
mise ran as the `dev` user. A Python HTTP server managed by systemd responded
on the guest loopback address.

The result does not provide a VM boundary. It is a system container sharing
the Colima Linux kernel. A Linux Host can run this without KVM. On macOS the
outer Colima Linux VM remains required. Incus VMs are a separate option, but
Colima documents that Incus VMs on Apple Silicon require an M3 or newer chip
because they need nested virtualization. See the [Colima Incus runtime documentation](https://colima.run/docs/runtimes/).

## Reproduction

The isolated profile was created with:

```sh
colima start self-host-incus-test --runtime incus --template=false \
  --mount none --activate=false --cpus 2 --memory 4 --disk 20
```

Colima reported `runtime: incus`, `cpus: 2`, `memory: 4GiB`, `disk: 20GiB`,
and an outer VM address of `192.168.64.3`. Its forwarded Incus socket was
`~/.colima/self-host-incus-test/incus.sock`. The client was configured with a
separate `INCUS_CONF` directory and an explicit remote for that socket. This
matters because the default macOS `local` remote points at a Unix socket that
does not reach the Incus daemon inside Colima.

The test launched an Ubuntu system container from the `images` remote:

```sh
incus launch images:ubuntu/24.04 self-host-incus-system
```

The image was an arm64 Ubuntu Noble image. The running container showed:

```text
Linux ... 6.8.0-117-generic ... aarch64
pid1=/sbin/init
systemctl is-system-running: running
```

An Incus proxy device forwarded the container's port 22 to port 22022 on the
outer Colima VM. SSH from the Mac host then worked through
`192.168.64.3:22022` using a temporary Ed25519 key. A temporary known-hosts
file and `StrictHostKeyChecking=yes` verified the host key during the check.
The container bridge address, `192.168.100.134`, was not directly routable
from the Mac host. A production runner therefore needs an explicit proxy or
another host-reachable forwarding path for copyable SSH instructions.

The native bootstrap was run twice with the saved mise configuration and an
HTTP service command. Both runs reached `SF_STEP ready`; the second run kept
the existing repository marker, systemd service, and versions file. After
both `incus restart` and an Incus stop/start, the marker remained and the
service was active. The bootstrap log was mode 0600 and the command output
contained progress milestones only.

One test setup detail exposed an idempotence requirement. A previous manual
mise installation had left `/home/dev/.local/state` owned by root. The first
bootstrap retry failed at `mise trust` with permission denied until the
directory was repaired for `dev`. The bootstrap must ensure ownership of the
complete user mise state tree before running mise, including when mise is
already installed.

## Storage and image behavior

The isolated Incus storage pool used the `zfs` driver and a 20 GiB backing
file. A per-instance root device override to `10GiB` succeeded, and the
relaunch container reported an 11G filesystem. This is enough to expose a
disk size control for this backend, subject to checking the selected pool's
quota support before accepting a requested size.

Publishing the configured system container and launching a new instance from
the resulting private image preserved all of the following:

* the installed `/usr/local/bin/mise` and its reported version;
* the `/home/dev/repos/personal-marker` file;
* the enabled systemd service and its configuration.

This demonstrates that an Incus image captures the current guest filesystem,
including personal data. A product update must keep the writable instance
disk authoritative and must not use publish/relaunch as its normal update
operation. If image based rebuilding is added later, repositories and
credentials need a separate persistent volume.

## OCI images are a different mode

The same Incus server launched `docker:alpine` as `CONTAINER (APP)`. It had an
Alpine userspace and a minimal `init`, rather than the Ubuntu systemd guest.
This mode can reuse an OCI image and its entrypoint, but it does not turn the
existing development image into a full native development guest. The T3
environment needs the Ubuntu system container path and its own bootstrap.

## Docker nesting

The Incus FAQ documents Docker in an Incus container with
`security.nesting=true`, and notes that some Docker configurations also need
host kernel modules. See [How can I run Docker inside an Incus container?](https://linuxcontainers.org/incus/docs/main/faq/#how-can-i-run-docker-inside-an-incus-container)
and [instance options](https://linuxcontainers.org/incus/docs/main/reference/instance_options/#security-nesting).

The disposable Ubuntu system container was configured with nesting and the
documented syscall interception flags:

```sh
incus config set self-host-incus-system \
  security.nesting=true \
  security.syscalls.intercept.mknod=true \
  security.syscalls.intercept.setxattr=true
```

`docker.io` installed, `docker.service` became active, Docker reported
`29.1.3 storage=overlayfs`, and `docker run --rm hello-world` completed.
This proves a nested Docker smoke test on this kernel. It does not prove all
self-host Docker, networking or filesystem tests will work. Nesting adds
shared-kernel security and resource concerns, and the required settings need
to be an explicit product decision. Incus recommends unprivileged containers
and warns that privileged containers can affect the host. See [Incus security](https://linuxcontainers.org/incus/docs/main/explanation/security/).

## Integrated prototype validation

After the feasibility test, the isolated `self-host-incus-test` profile was
recreated with `--network-address` for testing the implemented console and API.
One environment, `t3-incus-prototype`, remains running for Operator testing.
It has 2 CPUs, 4 GiB memory and a 10 GiB root disk limit. The production Colima
profile and Platform were not changed.

The real runner passed these checks:

* Create and retry use the same owned Incus instance.
* SSH works with a verified host key and the supplied Operator public key.
* Native T3, Node, Claude Code and Codex launch as `dev`; T3's native PTY exits
  successfully. The installed versions were T3 0.0.40, Node 24.21.0,
  Claude Code 2.1.268 and Codex 0.154.0.
* Browser pairing completes and T3 reports the environment connected.
* Stop, start, restart and repeated recipe application preserve a repository
  marker. T3 starts again through systemd.
* An update with a deliberately failing check reports failure while the previous
  T3 service remains ready. Correcting the recipe and retrying succeeds.
* Deleting a second disposable environment rejects the wrong confirmation name,
  then removes both the owned instance and its Platform record.
* Stopping the prototype Platform leaves SSH and T3 HTTP access working.
  Restarting it restores the saved environment state and readiness.
* The console displays real versions and access instructions without horizontal
  overflow at 1280 pixels. Demo lifecycle actions also passed browser checks.

Native Linux execution, provider sign-in, an authenticated agent task and the
full self-host Docker test suite inside this environment remain unverified.
The prototype does not enable Docker nesting. Its API harness and SSH tunnels
are temporary test processes, not an installed production service.

The Mac test exposed two CLI details now covered by a regression test:
`INCUS_REMOTE` must be set for the client process, and `incus list` takes the
remote and name filter as separate arguments. The runner preserves the global
default remote. See [Incus environment variables](https://linuxcontainers.org/incus/docs/main/environment/).

## Recommendation

Use Incus system containers for the first native T3 environment implementation
if the scope accepts shared-kernel isolation and the outer Colima dependency
on macOS. Keep the SSH proxy and host-key path as first-class runner state.
Expose Docker nesting as a separately checked capability, with explicit
security settings and truthful readiness reporting. Keep Lima or another VM
runner as the path for a genuine VM boundary or workloads that require kernel
isolation.
