# Names are records, and a machine joins the LAN

The Operator asked for a page to manage the Platform's DNS. An Application
gets a name when it is deployed; a Virtual machine gets a loopback port and an
address nobody can type. This decides what DNS the Platform keeps, what the
Operator can put in it, and how a machine gets an address a name can point at.

## The decision

Two decisions, delivered in this order.

**The zone is made of records the Operator can see and edit.** The Platform
keeps DNS records in Platform State under `dns_records_v1`, keyed by name and
type, each with a value, a fixed TTL of 60 seconds, a description and an
owner. A DNS screen in the console lists every record the Platform answers
for: the wildcard, `admin`, each Application's Hostname and Aliases, and the
records the Operator typed. Under the list it shows what the zone is built
from: the DNS Suffix, the Host addresses published right now, and the upstream
forwarders. The Operator creates and deletes `A` records through `POST` and
`DELETE /dns/records`, and edits through `PUT /dns/records/{name}/{type}`. An
explicit record wins over the wildcard, as DNS already says it should. A name
an Application answers on is refused, in both directions: a record cannot take
an Application's name and an Application cannot take a record's. The apex is
refused. `init` creates the `admin` record.

**A Virtual machine is a host on the LAN.** `install.sh` builds
`socket_vmnet` from source into `/opt/socket_vmnet` and installs a
LaunchDaemon that opens a bridged vmnet interface on the Host's LAN interface.
Every machine attaches to it with a MAC address the Platform derives from the
machine's identity and prints on its screen. The machine asks the LAN's own
DHCP server for an address. The Platform reads the lease from the guest's
`lima0` interface on each inspect and keeps an `A` record for
`<name>.<suffix>` pointing at it. `ssh dev@<name>.<suffix>` and
`http://<name>.<suffix>:<port>` reach the machine directly, without the proxy
and without the Platform running.

## The vocabulary changes with it

`CONTEXT.md` told writers to avoid "zone" as raw DNS jargon. That was right
while the Platform's DNS was one wildcard nobody edited. A screen that lists
records with types and TTLs is better described in the words DNS already has
than in invented ones, and inventing them now means renaming twice. The
glossary gains **Zone**, **Record**, **Record Type** and **TTL**. Application
Hostname, Hostname Alias and DNS Suffix stay: they name what a Consumer types,
which is a product concept, and one that a Record serves.

## What this amends

[ADR-0017](0017-host-native-dns.md) chose a wildcard zone with no per-name
records so the Platform would have nothing to keep in sync. The wildcard
stays and still answers for every name that has no record of its own. The
zone is no longer only the wildcard.

[ADR-0019](0019-embedded-http-proxy.md) and the doctrine in `src/ports.rs`
made the proxy the only thing on the Host a Consumer can reach; Web Targets
bind loopback. A machine on the bridged interface is reachable on every port
from every device on the LAN, and reaches every device on the LAN in turn, the
router included. The proxy is still the only door into an Application. It is
no longer the only door into the Host's network. This inverts an earlier
decision on purpose: the Operator asked for machines that behave like
machines, and a machine on a LAN is reachable on it.

[ADR-0023](0023-virtual-machines-run-on-apple-virtualization.md) left
"reaching a machine by name" open, and recorded that vmnet made
`mDNSResponder` take port 53. That finding was about the shared mode, which is
what Colima's `--network-address` uses. Bridged mode does not have that
property, and it had not been tried. This closes the question.

## What was verified before deciding

On the Host, an Apple Silicon Mac on Wi-Fi (`en0`, 192.168.1.54, gateway
192.168.1.1), macOS 26.6.2, `socket_vmnet` at `a061a81`, 2026-09-15:

* With the Platform stopped and port 53 free, starting
  `socket_vmnet --vmnet-mode=bridged --vmnet-interface=en0` left port 53
  free. `lsof` on TCP and UDP 53 was empty before and after. The daemon
  created `vmenet0` and `bridge100`; neither took a port.
* A throwaway `vz` instance attached to that socket with
  `macAddress: 52:55:55:be:ef:01` got `192.168.1.41/24` on `lima0` from the
  router's DHCP, with a default route through 192.168.1.1. The Host's ARP
  table showed that MAC. Lima kept its user-mode `eth0` at 192.168.5.15
  alongside it, at a higher metric, and its own SSH and agent still use it.
* The Host pinged the guest at 0.4 to 1.0 ms. Another machine on the LAN,
  over Wi-Fi, pinged it at 3.7 ms.
* `vmnet_copy_shared_interface_list()` on this Host lists `en0` as
  bridgeable. Bridging over Wi-Fi is the case that most often fails, because
  an access point usually accepts one MAC per association. This one did not
  fail.

The upstream record was read as well.
[lima-vm/lima#2176](https://github.com/lima-vm/lima/issues/2176) confirms the
port 53 behaviour for `vzNAT`, and its comments note that `mDNSResponder`
only takes 53 when it is free. Apple's `vmnet.h` in the macOS 26 SDK lists
the DNS proxy as a default of shared and host modes; it describes bridged mode
as traffic bridged to a physical interface, with no NAT, DHCP or DNS.

## What was rejected

**`vzNAT`**, Lima's built-in network on Apple Virtualization. The guest sits
behind NAT on 192.168.64.0/24 and no LAN device has a route to it. `vz` sets
`vmnet_enable_isolation_key`, so guests cannot reach each other either. And
it is shared mode, which is what takes port 53.

**`socket_vmnet` in shared mode.** Same subnet problem, same port 53 problem.

**`hostIP: 0.0.0.0` on Lima's port forwards.** It puts the forwarded ports
on the LAN without giving the machine an address. Every machine shares the
Host's IP, so a name distinguishes nothing; the random port still does. SSH
on port 22 works for one machine at most.

**A static address assigned by the Platform.** Lima generates the guest's
netplan with `dhcp4: true` and no override, so a static address would have to
be applied after boot by a provision script. It would also mean the Operator
declaring a slice of the LAN subnet at `init` that the router's DHCP pool does
not use, and the Platform probing ARP before every assignment. Three
mechanisms to avoid a lease, and a mistyped range takes an address from
another device in the house. DHCP is what the LAN already does. A stable
address is a reservation on the router, keyed by the MAC the Platform prints,
which is where the rest of the house's addressing already lives.

**Reaching machines through the proxy.** It was the first design, and it
gives a machine HTTPS for free. It also makes `ssh <name>` impossible, since
port 22 carries no name to route on, and it ties a machine's reachability to
the Platform running, which ADR-0023 flagged as the thing to avoid. The
Operator's requirement is SSH by name once machines run on the Host.

## How a machine keeps its address

The Platform pins the MAC and reads the lease. It does not write an address
anywhere the guest reads it from. A machine that stays stopped past the
router's lease time may come back with another address; the record follows on
the next inspect. A reservation on the router removes that window, and the
machine's screen prints the MAC to key it on.

Lima's loopback port forward for `web_port` stays, for one reason:
`answering()` in `src/vms.rs` probes it, and between boot and the first lease
that probe is the only thing that answers. The `web_url` the Operator copies
becomes `http://<name>.<suffix>:<web_port>`.

## What this costs

Root, once. `install.sh` already uses `sudo` for LaunchDaemons and
`/etc/resolver`; it gains a build of `socket_vmnet` from source, because the
Lima project does not recommend the Homebrew package, and one more
LaunchDaemon. The daemon runs unmanaged, always up, so the Platform never
needs `sudo` at runtime and there is no passwordless grant to audit.

`socket_vmnet` has open issues that matter for a household server.
[#173](https://github.com/lima-vm/socket_vmnet/issues/173): one client that
stops draining stalls the forwarding loop for every machine on the daemon.
[#130](https://github.com/lima-vm/socket_vmnet/issues/130): routes lost after
wake from sleep on Apple Silicon.
[#36](https://github.com/lima-vm/socket_vmnet/issues/36): no L2 learning, so
every packet reaches every client.

`admin` is an ordinary record. `init` creates it with the Host's address at
that moment, and the Operator can edit or delete it. When the Host's address
changes, `admin.<suffix>` points at the old one until the Operator edits the
record, and the console is unreachable by name in between. The wildcard,
rebuilt from the Host's interfaces every 30 seconds, does not have this
problem. The Operator chose an editable record over a managed one, knowing
this.

A machine's SSH host key is regenerated when the machine is recreated. With a
stable name, `known_hosts` keeps the old key under that name and `ssh`
refuses the new one until the Operator runs `ssh-keygen -R <name>.<suffix>`.
The random port hid this until now.

## What stays out of scope

IPv6. The wildcard publishes `A` records only, `host_addresses.rs` collects
IPv4 only, and Operator records accept type `A` only. The zone is keyed by
type so `AAAA` slots in without a change of shape, and
[#82](https://github.com/momoi-labs/self-host/issues/82) is where that work
lives. The Host has global IPv6 today, so this is a known gap, not an
oversight.

TLS on a machine. A name that resolves to the machine bypasses the proxy and
the wildcard certificate with it. A per-machine certificate from the
Platform's CA, installed in the guest, is the right answer and separate work.

Persisting a machine's SSH host key across recreation. A firewall between
machines and the LAN. Multiple DNS Suffixes. Record types other than `A`.

**Status:** accepted

**Extends:** [ADR-0023](0023-virtual-machines-run-on-apple-virtualization.md),
closing its open question on reaching a machine by name.
**Amends:** the wildcard-only zone in [ADR-0017](0017-host-native-dns.md),
the proxy as the sole LAN entry in [ADR-0019](0019-embedded-http-proxy.md),
and the DNS vocabulary in `CONTEXT.md`.
**Context:** the DNS epic, [#118](https://github.com/momoi-labs/self-host/issues/118). IPv6 is
[#82](https://github.com/momoi-labs/self-host/issues/82).
