# A Virtual machine is a virtual machine

The Operator asked for virtual machines: persistent Linux workspaces with
their own tools, their own repositories, and access that does not depend on
the Platform being up. This decides what runs underneath that word.

## The decision

One Lima instance per Virtual machine, on Apple's Virtualization framework.

The Host creates it from Canonical's Ubuntu cloud image, sizes its CPU, memory
and disk, provisions it with cloud-init, and manages its life through
`limactl`. The console opens a shell with `limactl shell`. Lima runs the same
instance on `qemu` when the Host is Linux, so one runner serves both.

## What this replaces, and why

This file first recorded the opposite: Incus system containers sharing the
Host's Linux kernel, on the grounds that a real VM would need nested
virtualization that macOS could not provide. That premise was wrong on the
Host we have, and the rest followed from it.

Nested virtualization works on this Mac. An Incus VM booted with `--vm`,
reported `systemd-detect-virt: kvm`, and ran its own kernel. Once that was
established, the shared kernel bought nothing and cost the truth of the
feature's own name.

The container also lied about what it was given. A machine configured with
`limits.memory=8GiB` on a 4GiB host saw 3904MB, because a cgroup caps a
container without reserving anything, and the container reads the host's
memory. The form promised CPU, memory and disk that never existed. A VM
allocates its memory at boot and fails loudly when it will not fit.

The Operator never accepted the shared kernel. They said so the first time the
screen was shown, and the record above claimed otherwise.

## Why Lima and not Incus with `--vm`

Incus needs a Linux host. On a Mac that means booting a Linux VM to run the
Incus daemon so that Incus can boot a VM. Three layers to deliver one machine.
[ADR-0013](0013-macos-bootstrap-with-launchd.md) makes the first Host an Apple
Silicon Mac used as a household server, so that stack is not a development
detail; it is the product.

Lima drives Apple's hypervisor directly. The guest reports
`systemd-detect-virt: apple`, one layer above the hardware.

Writing against the framework ourselves would mean building image handling,
disk management, port forwarding and a lifecycle API. Lima has them, is already
a dependency of the Colima this project installs today, and carries the
`com.apple.security.virtualization` entitlement it needs.

## What was verified before deciding

On `gastly`, an Apple Silicon Mac, 2026-09-12:

* A Lima instance reports `systemd-detect-virt: apple` and runs kernel
  `6.8.0-134-generic`, separate from the Host.
* It boots in 38 seconds from creation and 11 seconds from a stopped disk.
* `journalctl -b` inside carries 1850 lines of kernel boot.
* A LaunchDaemon in the `system` domain, running as the Operator with no
  graphical session, started it and reported `exit=0`. This is the condition
  ADR-0013 requires and the one most likely to refuse a hypervisor.
* Secure boot never arises. Lima does not enable it, so the signature failure
  that `--vm` hit under Incus has nothing to reject.
* Removing the Colima profile freed port 53. Its `--network-address` put the
  Mac on the shared vmnet network, which makes `mDNSResponder` listen there
  and collide with the Platform's own DNS. Lima's user network does not.

## What provisioning looks like now

cloud-init, through the `cidata.iso` Lima builds from a cloud-config. It runs
during boot rather than after it, which is how a machine image is meant to be
prepared, and it survives a Platform restart without a shell session held open
across it.

The cost is the progress protocol. The current bootstrap script prints
`SF_STEP` markers that drive the phases on screen; cloud-init has no such
channel, so progress has to be read from the guest's own log. The Operation
tab is being replaced by boot and provisioning logs for this reason.

## What this costs

Every existing prototype machine. Incus instances, their disks and the Colima
profile that hosted them are gone, and nothing migrates. The prototype has no
users other than the Operator, who asked for the change.

Boot takes tens of seconds where a container took one. A VM reserves the memory
it is given instead of overcommitting it, so a Host runs fewer machines than
the container count suggested. Both are the honest price of the name.

## What stays out of scope

Reaching a machine by name. A machine answers on a forwarded port today, and
whether it should be published through the Platform's proxy and DNS, which
would tie its reachability to the Platform running, is still open.

Other base images, snapshots, and migration between Hosts.
