# A suffix inside the public zone gets one wildcard

When the saved deSEC zone equals or contains the DNS Suffix, the Platform
orders one certificate for `*.<suffix>` and `<suffix>` through DNS-01, and
serves it for every name one label under the suffix: Applications, aliases,
route rules, `console` and `admin`. We call this public suffix mode. It is
off unless the Operator saves such a zone, in Settings or in the installer.

## Why one wildcard

A Host in public suffix mode has many short-lived names. One certificate per
name means one order and one nameserver wait per Application, and a new
Application waits up to 10 minutes for its first certificate. The wildcard is
issued once, renews once, and covers a new Application at once. It needs
DNS-01: ACME has no other challenge for a wildcard.

Both identifiers in the order write to `_acme-challenge.<suffix>`. The
Platform writes both values into one TXT record set and waits until every
authoritative nameserver serves both.

## The LAN leaf is the fallback

Before the first issuance and after a failure, names under the suffix get the
LAN leaf, signed by the local CA, as they did before. Devices that trust the
CA keep working while the wildcard is missing. This is different from a
managed public Hostname outside the suffix, which refuses the handshake
without its own certificate.

## Scope

- Only one label is covered. `a.b.<suffix>` keeps the LAN leaf, because a
  wildcard covers one label only.
- An explicit Hostname with its own certificate keeps it. The resolver tries
  the exact name first, then the wildcard, then the LAN leaf.
- Names outside the suffix keep their own certificate through HTTP-01 or
  DNS-01, as in ADR-0036.
- Wildcards in `public.json` `hostnames` are still refused. The only wildcard
  is the one the Platform derives from the suffix.

**Status:** accepted

**Supersedes:** "no wildcard" in the scope of
[ADR-0036](0036-public-certificates-can-use-dns-01-through-desec.md).
