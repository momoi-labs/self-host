# Virtual machines

A Virtual machine is a Lima instance on Apple's Virtualization framework
([ADR-0023](adr/0023-virtual-machines-run-on-apple-virtualization.md)). It runs
Ubuntu on its own kernel, with systemd, mise and T3 as native processes. The
Platform manages only instances named `sf-dev-<id>`.

## What the Host needs

```sh
brew install lima
```

Lima is signed with the `com.apple.security.virtualization` entitlement and
runs the machine directly on the hypervisor, so there is no Linux host in
between. On a Linux Host, Lima uses `qemu` instead of `vz`, and the same runner
serves both.

The other half is the network the machine joins. `install.sh` builds
`socket_vmnet` and installs a LaunchDaemon that bridges a vmnet interface onto
the Host's LAN interface, so a machine is a device on the LAN with an address
of its own ([the bridged network](macos-host.md#the-bridged-network-for-machines)).
That daemon runs as root, unmanaged, from boot; the Platform never needs
`sudo` for it.

`SELF_HOST_LIMA_BIN` overrides the executable. Otherwise the Platform uses
`/opt/homebrew/bin/limactl` on macOS when it is there, then `limactl` from
`PATH`.

## What a create does

The Host writes an instance template carrying the Operator's CPU, memory and
disk, the bridged network with a MAC address the Platform pins to the machine,
a forwarded port for the web service, and the provisioning script. Lima
downloads Canonical's Ubuntu 24.04 cloud image, verifies its digest, boots it,
and cloud-init runs the script during that boot.

The machine's name becomes its DNS name, `<name>.<suffix>`, so it has to be a
DNS label: a create refuses a name with a space or any other character a label
cannot carry, and a name an Application, a Record or another machine already
answers on ([ADR-0026](adr/0026-a-machine-is-its-record-a-record-can-move-and-a-name-is-a-label.md)).

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

A machine is a host on the LAN
([ADR-0025](adr/0025-names-are-records-and-a-machine-joins-the-lan.md)). It
asks the router's DHCP server for an address like any other device in the
house, the Platform reads the lease from the machine's `lima0` interface on
each inspect, and keeps an `A` Record for `<name>.<suffix>` pointing at it.
The machine's screen prints the three things you need, under its title: the
name, the address the lease gave it, and the MAC.

From any device that uses the Host as its DNS server
([configure another device](macos-host.md#configure-another-device)):

```sh
ssh dev@<name>.<suffix>
```

The web service answers on `http://<name>.<suffix>:<web_port>`, the link the
screen shows for a machine that has one. Neither path goes through the
proxy. The Platform's part is the name. Once the device has resolved it, the
traffic runs between the device and the machine, and a session that is open
when the Platform daemon stops keeps running. A new lookup waits for the
daemon to be back.

The web service has to listen on every interface. A loopback listener inside
the machine is reached by Lima's forward to the Host's loopback, which the
Platform keeps for its own readiness probe, and by nothing on the LAN.

The Connect to console tab does not depend on the name. It opens a shell
inside the machine as `dev` through the Host.

### Keeping the address

Reserve the address on the router, keyed on the MAC the machine's screen
prints. The address is a DHCP lease, and a machine that stays stopped past the
router's lease time can come back with another one. The Record follows on the
next inspect; a bookmark or a firewall rule that memorized the old address does
not.

The Platform derives the MAC from the machine's record, not from its name. A
retried create keeps it. Deleting the machine and creating another with the
same name is a new record, a new MAC, and a new reservation to make.

### After recreating a machine

`ssh` refuses a machine that was deleted and created again under the same
name, or created again after Lima lost its instance. The new disk has a new
SSH host key, and `known_hosts` on every device that connected before still
holds the old one under the same name:

```text
WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!
```

Forget the old key on the device that connects, then connect again:

```sh
ssh-keygen -R <name>.<suffix>
ssh dev@<name>.<suffix>
```

The random port hid this until now; a stable name shows it.

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
