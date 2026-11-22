# An External Application fronts a LAN service

An External Application is an Application whose Runtime is a service that
already runs on the LAN, outside the Platform. The Platform runs nothing for
it. The proxy gives it a Hostname, aliases, DNS, a certificate, route rules
and redirects, the same as any Application, and forwards each request to its
upstream. The first case is the UniFi gateway at `https://192.168.1.1`, which
presents a self-signed `CN=unifi.local` certificate.

The definition is `{"kind":"external","upstream":"https://192.168.1.1","pin":"<SHA-256>"}`.
The upstream is `http` or `https`, an IP literal and an optional port. The pin
is optional.

## Which upstreams are allowed

The upstream must be an RFC 1918 IPv4 address or an IPv6 ULA. The API refuses
these, and the message names the rule:

- loopback, because that is where Applications run, and an External
  Application would reach around their isolation;
- link-local, multicast and unspecified addresses;
- public addresses, because the proxy must not become an open relay off the
  LAN;
- names, because DNS can point a name at any address after the check;
- the Host's own addresses, because the proxy would send requests back to
  itself;
- Docker bridge networks (`172.17.0.0/16` and the subnets of `docker*` and
  `br-*` interfaces), where Applications run.

## How the upstream certificate is checked

Without a pin, the proxy checks the chain against the system roots and the
name against the upstream IP, as a browser would. A self-signed device
certificate fails this check, and the 502 says to pin it.

With a pin, the proxy compares the SHA-256 fingerprint of the leaf certificate
with the pin, and ignores the chain and the name. It still checks the
handshake signature, so a peer must hold the private key of the pinned
certificate. A mismatch is a 502 that names the mismatch and a log line with
the expected and presented fingerprints.

No option turns verification off. A pin is the only way to accept a
certificate the system roots do not trust, and it accepts one certificate.

The proxy offers only `http/1.1` in ALPN, so WebSocket upgrades pass through
as on a loopback target.

## Host header

The proxy forwards the client's `Host`, as it does for every Application.
UniFi checks `Origin` against `Host` on WebSocket upgrades. With the default,
a browser's `Origin` matches and the upgrade reaches UniFi's own auth. With
`rewrite_host`, `Host` becomes `192.168.1.1:443`, the check fails, and UniFi
answers 500. Leave `rewrite_host` off for UniFi.

## Scope

- An External Application has no containers, logs, environment, terminal or
  database connection. Those endpoints refuse it or return nothing. Start and
  stop publish and withdraw its route.
- Deploying or changing one needs the `host` scope, because it opens a path
  from the proxy to a LAN address.
- The console creates, edits and removes one from Routes. It fetches the
  leaf certificate with `POST /external/fingerprint`, which needs the `host`
  scope and applies the same address rules. The fetch trusts any
  certificate, so the Operator confirms the fingerprint against the device
  before the console saves it as the pin.
- Not covered: mTLS to the upstream and upstreams by DNS name.

## Consequences

A pin breaks when the upstream makes a new certificate, for example after a
firmware reset. The proxy then answers 502 until the Operator saves the new
fingerprint.

**Status:** accepted
