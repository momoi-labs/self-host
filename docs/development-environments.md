# Virtual machines

A Virtual machine is a Lima instance on Apple's Virtualization framework
([ADR-0023](adr/0023-virtual-machines-run-on-apple-virtualization.md)). It runs
Ubuntu on its own kernel, with systemd, mise and T3 as native processes. The
Platform manages only instances named `sf-dev-<id>`.

## What the Host needs

```sh
brew install lima
```

That is the whole prerequisite on macOS. Lima is signed with the
`com.apple.security.virtualization` entitlement and runs the machine directly
on the hypervisor, so there is no daemon to configure and no Linux host in
between. On a Linux Host, Lima uses `qemu` instead of `vz`, and the same runner
serves both.

`SELF_HOST_LIMA_BIN` overrides the executable. Otherwise the Platform uses
`/opt/homebrew/bin/limactl` on macOS when it is there, then `limactl` from
`PATH`.

## What a create does

The Host writes an instance template carrying the Operator's CPU, memory and
disk, a forwarded port for the web service, and the provisioning script. Lima
downloads Canonical's Ubuntu 24.04 cloud image, verifies its digest, boots it,
and cloud-init runs the script during that boot.

The script installs system packages, creates the `dev` user with the Operator's
SSH key, installs mise and the recipe's tools, writes the
`self-host-environment` systemd unit and waits for the web service to answer.
It prints `SF_STEP` markers, which the Host follows over SSH while the boot
runs, so the console shows the phase the machine is in.

Nothing of the Host is mounted into a machine and containerd is off. This is a
workspace, not a container runtime.

Create takes a few minutes on a first run, most of it installing tools.
Booting an existing machine takes seconds.

## Reaching a machine

The web service answers on a port Lima forwards to the Host's loopback,
allocated when the machine is created. The console shows the address, and the
Connect to console tab opens a shell inside the machine as `dev`.

Publishing a machine under the DNS Suffix, so it answers on a name like an
Application does, is still open.

## What the machine reports

The Logs tab reads three sources. **Boot** is `journalctl -b` inside the
machine, the kernel and systemd coming up. **Provisioning** is
`/var/log/cloud-init-output.log`, what cloud-init ran. **Platform** is the
Host's own event log under `environments/<id>.log` in Platform State, which is
the only one that survives a machine that never booted.

Resource use is measured from inside the machine, through `/proc`, because the
Host only sees one hypervisor process. It appears on the Overview beside the
Applications and on the machine's own screen.

## Lifecycle and data

Create boots one instance, sizes it and provisions it. Repeating create finds
the existing instance first, so an interrupted one continues. Start, stop and
restart keep the disk. Bootstrap and update run the provisioning script again
through a shell, because cloud-init only fires on the first boot.

Update does not upgrade Ubuntu, resize the machine, or uninstall tools dropped
from the recipe.

Repositories, credentials and T3 state live on the machine's disk. Delete
removes that disk with the machine, and takes its event log with it.

## Pairing T3

After opening the terminal, run
`t3 pair --base-dir /home/dev/.local/state/t3` and paste the token into T3.
Sign agents in through their own CLI inside the machine. Pairing tokens and
provider credentials never belong in a saved recipe.
