# Public certificates can use DNS-01 through deSEC

A Hostname inside a DNS zone the Operator saved in Settings is validated with
DNS-01. The Platform writes the `_acme-challenge` TXT record through the deSEC
API, waits until every authoritative nameserver serves it, and deletes it
after the order. Every other Hostname keeps HTTP-01. DNS-01 is opt-in and the
local CA stays the default.

## Why deSEC

The test zone `int.momoi.cloud` is delegated from Cloudflare to deSEC
(`ns1.desec.io`, `ns2.desec.org`). A Cloudflare API token covers the whole
zone, and a child zone needs Enterprise, so a token that can only write
challenge records is not available there. A deSEC token can be limited to TXT
records in one domain. The check: a TXT write returned 200 and an A write
returned 403.

## Why the Platform asks every nameserver

On 2026-11-19, `ns1` served a new TXT in about 3 seconds. `ns2` lagged about 4
minutes, and served a deleted TXT 2 minutes after removal. A recursive
resolver also cached a negative answer. The Platform therefore queries each
authoritative nameserver directly, never a recursive resolver, and waits for
all of them for up to 10 minutes. The order poll has the same limit.

The Platform does not wait for removal. It sends `DELETE` and moves on. A stale
TXT cannot break the next order, because the write is a `PUT` that replaces the
record set.

## Scope

One certificate per Hostname, no wildcard, one provider, no provider
abstraction. [ADR-0038](0038-a-suffix-inside-the-public-zone-gets-one-wildcard.md)
replaces "no wildcard" for a DNS Suffix inside the zone. The token lives in `public.json` with mode `0600`. The settings
API reports only `token_set`.

What this costs: a second provider needs code, and a zone whose nameservers
stay behind for 10 minutes fails the order with "challenge record did not
reach the nameservers".
