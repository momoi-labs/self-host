# A machine is its record, a Record can move, and a name is a label

[ADR-0025](0025-names-are-records-and-a-machine-joins-the-lan.md) decided
that the Zone is made of Records and that a Virtual machine is a host on the
LAN. Six slices implemented it (#120 to #125). The conformance review that
closed the epic ([#126](https://github.com/momoi-labs/self-host/issues/126))
walked every story against the merged code and found four places where a
slice read the decision in a way the ADR did not state. The Operator accepted
each while validating the slice. This records them, so the reading is a
decision and not an accident of the code.

## A machine's identity is its record, not its name

ADR-0025 says the Platform derives a machine's MAC address "from the
machine's identity"; #123 asked for one "stable across recreation of the same
machine" and, in its acceptance criteria, that "deleting and recreating a
machine yields the same MAC". The code derives it from the machine's durable
record ID
(`mac_address` in `src/vms.rs`), the `env-...` identifier that never changes
for as long as the record exists. Recreating the Lima instance behind the same
record, after a failed create or a retry, keeps the MAC. Deleting the machine
and creating another one with the same name is a new record, a new MAC, and a
new reservation to make on the router.

The alternative, deriving the MAC from the name, would let a deleted machine's
reservation pass to whatever takes its name next, which is the kind of
surprise a household router should not hand out. The Operator confirmed the
reading in [#130](https://github.com/momoi-labs/self-host/pull/130). The
cost is that a machine's screen has to print the MAC where the Operator can
read it before reserving. The Operator had #125 drop it when the address moved
into the page header, judging it noise; the review brought it back, next to
the hostname and the lease, because the reservation is the Operator's only
handle on a stable address, and the Operator agreed.

## A Record can be renamed in place

ADR-0025 lists `PUT /dns/records/{name}/{type}` as the way to edit a Record's
value and description. The DNS screen (#124) needed a rename, and deleting
and recreating a Record would lose its audit identity and leave the wildcard
answering for a moment in between. `PUT` therefore accepts an optional `name`.
When it differs from the key in the path, the Record moves to the new key under
the same namespace check a new Record gets, the served Zone publishes the new
name before it withdraws the old, and the audit event's subject is the new
key (`UpdateRequest` and `update` in `src/dns_records.rs`). Omitting `name`
keeps the existing contract. The console offers the new name for Operator
Records only; `admin` keeps its name there, so the row stays where the
Operator looks for it.

## A machine's name is a DNS label

ADR-0025 gives every machine the hostname `<name>.<suffix>`. That makes the
machine's name a DNS label whether or not anyone says so, and the code says
so: a name with a space or any other character a label cannot carry is refused
at create with 400 (`default_hostname` in `src/environments.rs`). Before the
epic a machine's name had no such rule.

Machines created before names get a hostname the first time the daemon loads
them after a start, under the same namespace check a new machine gets. A
machine whose name is not a label, or whose name is already answered by
something else, keeps an empty hostname; its `ssh_command` and `web_url` stay
on Lima's loopback port. The daemon tries again on its next start, so renaming
the machine to a label and restarting names it.

## Published Host addresses come from the Zone

`GET /bootstrap/status` reported `host_addresses` as whatever the Host's
interfaces held at that moment, scanned on each call. The DNS screen shows the
same field as "Published Host addresses", and the two could disagree for up to
30 seconds. The field now reads the addresses the served Zone's wildcard
publishes (`addresses` on the `Zone` trait). Under `serve`, that is the
interface scan the Zone already runs every 30 seconds. Under `serve --no-dns`
nothing is published, so the field is empty, and the bootstrap screen shows
no addresses in that mode. That is the truth of the matter rather than a
regression: a development daemon that serves no DNS publishes no address.

## What the review fixed on the way

Four gaps were small enough to close with this record rather than track. The
machine's screen prints the MAC again. The DNS screen lets the Operator edit
and delete `admin`, which #124 had locked as a Platform row while the API and
the ADR let it through. The screen's origin column says the Application's or
machine's name where it said `App/` and `VM/`, words the glossary does not
use. And `docs/operating.md` documents the four `/dns/records` routes, their
`owner` values and their refusals, which no slice had written down.

## What this does not settle

The bridge has not been checked through a reboot with nobody logged in or
through `uninstall.sh` on the Host; both need a person there and are
[#134](https://github.com/momoi-labs/self-host/issues/134).
Acceptance from a second device on the LAN, and the Operator documentation
for reaching a machine by name, stay with the epic's final slice (#118).

**Status:** accepted

**Amends:** the MAC derivation, the `PUT` contract, and the machine naming
rule in [ADR-0025](0025-names-are-records-and-a-machine-joins-the-lan.md).
The fourth reading, published Host addresses, touches no earlier decision.
**Context:** the conformance review,
[#126](https://github.com/momoi-labs/self-host/issues/126), for the DNS epic,
[#118](https://github.com/momoi-labs/self-host/issues/118).
