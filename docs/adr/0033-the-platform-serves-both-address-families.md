# The Platform serves both address families

The Platform publishes, listens and forwards on IPv4 and IPv6 alike
([#82](https://github.com/momoi-labs/self-host/issues/82)). A Host with both
families answers every name in the Zone with A and AAAA records. A Host with
only IPv6 initializes, serves and forwards with no IPv4 address at all.

## Which IPv6 addresses are published

IPv6 follows the rule IPv4 already has (#61): the addresses on the subnet of
the address the default route leaves through, filtered by the Operator's
`include` and `exclude`, which now take addresses of either family. Each
family is anchored on its own default route.

An IPv6 interface carries more addresses than an IPv4 one. macOS and most
Linux desktops add privacy addresses that rotate every day, and the default
route usually leaves through one of them. A privacy address still anchors the
subnet, but it is never published, and neither is an address that is
deprecated, tentative or failed duplicate detection. What remains is one
stable address per interface. The flags come from `/proc/net/if_inet6` on
Linux and from `SIOCGIFAFLAG_IN6` on macOS, since `getifaddrs` carries
neither.

A ULA prefix on the same interface sits on another subnet and is left out,
as an off-subnet IPv4 address is. The Operator adds it with `include`. The
provider's prefix can change; the 30-second scan follows it the way it
follows a DHCP change.

## A name with a Record answers for itself

Hickory falls back to the wildcard one record type at a time. Once the
wildcard carries AAAA, an A Record for `nas` would answer AAAA with the
Host's own address, and a dual-stack client, which prefers IPv6, would reach
the Host instead of the NAS. So a name that has Records answers only the
types it carries, and NODATA for the rest, as RFC 4592 has it. The same holds
for a Virtual machine's name.

`admin` is the exception. Its Record is the Host's, so it takes the
wildcard's AAAA and follows the Host's IPv6 addresses through every scan. On
an IPv6-only Host `init` creates no `admin` Record, and the wildcard answers
for it.

Record Types stay `A` only. An Operator AAAA Record, and listing the
wildcard's AAAA answers in the Records inventory, are follow-up work.

## Listeners

DNS, HTTP and HTTPS each add a listener on the IPv6 unspecified address,
IPv6-only, next to the IPv4 one. On macOS the unspecified address is the only
one the Operator may bind below port 1024, for IPv6 as for IPv4
(`in6_pcbbind`). On Linux a named IPv6 address would go stale with the
provider's prefix. The IPv6 listener is best effort when IPv4 has one: a Host
whose IPv6 port is taken keeps serving IPv4, with a warning. On an IPv6-only
Linux Host the IPv6 DNS listener is the required one.

## Forwarding

The forwarders are Cloudflare's IPv4 and IPv6 pairs. An IPv6-only Host
cannot reach the IPv4 pair, and the forwarder moves on to the IPv6 one. The
Platform does not synthesize AAAA records for IPv4-only names (DNS64); a
network that needs it already runs it.

**Status:** accepted

**Amends:** the IPv6 exclusion in
[ADR-0025](0025-names-are-records-and-a-machine-joins-the-lan.md), and the
listen addresses in [ADR-0017](0017-host-native-dns.md) and
[ADR-0019](0019-embedded-http-proxy.md).
