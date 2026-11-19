# Public certificates

The Platform issues one public certificate for every Hostname outside the
DNS Suffix, through HTTP-01 or, inside a saved deSEC zone, DNS-01: an Application's Hostname, its aliases and the Hostnames of its
route rules. Names under the Suffix keep the Platform's own CA, unless the
Suffix is inside the saved zone (public suffix mode, below). The ACME
account and the private keys live in Platform State; an Application never
receives them. Issuance is off until the Operator saves an account.

## Configure the account

In Settings, the Public certificates card asks for the certificate authority
and a contact email. Saving accepts the CA's terms of service. Use Let's
Encrypt staging first on a Host you can throw away; staging certificates are
not trusted by ordinary browsers. The Hostnames' public DNS must point at the
Host, and the CA must reach its HTTP listener on port 80.

The API behind the card:

| Request | Result |
| --- | --- |
| `GET /certificates/acme` | `{"directory_url", "email", "dns"}`, or `null` while issuance is off. `dns` is `null` or `{"provider", "zone", "token_set"}`. The token is never returned. |
| `PUT /certificates/acme` with `directory_url`, `email` and optional `dns` | Saves the account. The directory must be `https://`. `dns` absent keeps the saved zone, `null` removes it, and a missing `token` keeps the saved one. |
| `DELETE /certificates/acme` | Turns issuance off. Certificates and the account stay on disk. |
| `GET /certificates` | One status per managed Hostname (below). |

The daemon reads the account and the Applications' Hostnames again once a
minute, so a saved account, a new Hostname or a removed one needs no restart.
A Hostname that leaves every Application stops being served; its certificate
stays on disk and serves again if the name comes back. Saving a different CA
starts a new account; the certificates the old CA issued keep serving until
the new one renews them.

The card writes `state/certificates/public.json`. The file can also be written
by hand, with mode `0600`, which is how a private ACME test CA is configured:

```json
{
  "directory_url": "https://acme-staging-v02.api.letsencrypt.org/directory",
  "contact": ["mailto:operator@example.invalid"],
  "hostnames": ["alias.other.invalid"],
  "ca_cert_path": "/path/to/test-ca.pem"
}
```

`hostnames` lists names issued besides the Applications' own, and may be
empty or absent. `ca_cert_path` is a PEM trust anchor for a private CA; the
Platform verifies the ACME service's HTTPS certificate either way. Saving
from the card keeps both. Hostnames must be explicit DNS names. Wildcards, IP
addresses, duplicates and paths are refused. The only wildcard is the one
public suffix mode derives from the DNS Suffix.

## DNS validation

A Hostname inside a zone saved in Settings is validated with DNS-01. It needs
no inbound port 80. Hostnames outside the zone keep HTTP-01. The provider is
deSEC, and each Hostname outside the DNS Suffix gets its own certificate.

Set it up:

1. Delegate the zone to deSEC, for example `int.momoi.cloud` with
   `ns1.desec.io` and `ns2.desec.org`.
2. In deSEC, create a token limited to TXT records in that domain.
3. In Settings, Public certificates, fill in DNS zone and deSEC token, then
   save. The saved card shows that a token is set and never shows it.

Saved by hand, the file adds a `dns` object. The file keeps mode `0600`:

```json
{
  "directory_url": "https://acme-staging-v02.api.letsencrypt.org/directory",
  "contact": ["mailto:operator@example.invalid"],
  "dns": {"provider": "desec", "zone": "int.momoi.cloud", "token": "<token>"}
}
```

For each order the Platform writes `_acme-challenge.<hostname>` with a `PUT`,
which replaces any stale record. It then asks every authoritative nameserver
directly, never a recursive resolver, and waits up to 10 minutes until all of
them serve the value. The order poll also waits up to 10 minutes. It sends a
`DELETE` after the order and does not wait for removal.

Two card errors are fixed text. "DNS provider refused the challenge record"
means deSEC rejected the write, usually a wrong token or zone. "Challenge
record did not reach the nameservers" means a nameserver never served it
within 10 minutes.

Why every nameserver: in the test zone `ns1` served a new TXT in about 3
seconds, while `ns2` lagged about 4 minutes and served a deleted TXT 2 minutes
after removal. A recursive resolver also cached a negative answer. See
[ADR-0036](adr/0036-public-certificates-can-use-dns-01-through-desec.md) for
this and for why deSEC and not Cloudflare: a Cloudflare token covers the whole
zone and child zones need Enterprise, while a deSEC token can be limited to
TXT in one domain.

## Public suffix mode

When the saved zone equals or contains the DNS Suffix, for example zone
`momoi.cloud` or `int.momoi.cloud` with Suffix `int.momoi.cloud`, the Platform
orders one certificate for `*.int.momoi.cloud` and `int.momoi.cloud`. It
serves that certificate for every name one label under the Suffix:
Applications, aliases, route rules, `console` and `admin`. `GET /certificates`
lists it as `*.int.momoi.cloud`.

- Both identifiers share `_acme-challenge.<suffix>`. The Platform writes both
  values into one TXT record set and waits for every nameserver to serve both.
- Before the first issuance and after a failure, names under the Suffix get
  the LAN leaf from the local CA. Devices that trust the CA keep working.
- A name two labels down, such as `a.b.int.momoi.cloud`, is not covered by a
  wildcard and keeps the LAN leaf.
- An explicit Hostname with its own saved certificate keeps it.

The installer can turn this on: it asks whether to use a public DNS zone, and
the answer defaults to no. See
[ADR-0038](adr/0038-a-suffix-inside-the-public-zone-gets-one-wildcard.md).

## State and renewal

The authoritative `state/certificates` directory holds:

| File | Contents |
| --- | --- |
| `public.json` | CA directory, contact, managed Hostnames and the optional DNS zone and token |
| `account.json` | CA directory and serialized ACME account credentials |
| `<hostname>.json` | One certificate chain and its matching private key, `*.<suffix>.json` for the wildcard |
| `status.json` | Expiry, renewal, attempts, retry times and errors |

Directories have mode `0700`. Files have mode `0600`. The Platform replaces
each JSON file atomically and syncs it to disk. A chain and its key share one
file, so a crash cannot pair a new certificate with an old key. Symlink state
files and symlink certificate directories are refused. Back up this directory
with Platform State. The generated LAN CA remains in its existing location.

Renewal starts in the last third of the certificate's lifetime, capped at
30 days before expiry. A failed issuance or renewal retries after one hour.
Each Hostname has its own order and retry state. A failed alias does not block
another name. A failure keeps the previous certificate until it expires.
Successful renewal replaces the live SNI entry without restarting listeners
or Applications. Restart reloads the saved account and certificate bundles.

`GET /certificates` is an authenticated Operator API. It returns metadata,
including `hostname`, `state`, `not_before`, `expires_at`, `renew_at`,
`last_attempt`, `retry_at` and `last_error`. Times are Unix seconds. States are
`pending`, `valid`, `renewal_failed` and `expired`. Keys, PEM chains, account
credentials and challenge responses are absent. Logs report the failing
Hostname; the API records a bounded error without copying CA response bodies.
The Settings card shows the same list, refreshed every fifteen seconds.

## TLS selection and challenges

An explicitly managed public Hostname receives only its exact saved
certificate. A missing, expired or not-yet-valid certificate refuses the TLS
handshake. The proxy does not replace it with the LAN wildcard.

In public suffix mode, a name one label under the Suffix receives the public
wildcard while it is valid, and the LAN leaf otherwise.

Every other name, and a client without SNI, receives the Platform's own
certificate, whether or not its names cover the one asked for. A browser
reaching the Host directly rejects the mismatch, as it would have rejected a
refused handshake. An edge proxy that trusts the origin on its own terms,
such as Cloudflare in its "Full" mode, completes the handshake and serves the
Application with no public certificate at all; that is the setup for a Host
whose domains all sit behind such a proxy, and it needs no ACME account. The
HTTP Hostname check still controls access to management and Application
routes.

The proxy answers `/.well-known/acme-challenge/{token}` before redirects,
management routes and Application rules on both listeners. A response is
bound to the exact Hostname and token. Unknown, completed and encoded tokens
return `404`. The Platform removes active tokens after an order finishes or
fails. Challenge tokens never enter persisted Application configuration.

## Local evidence and staging blocker

Run the certificate suite from an implementation checkout with a built console:

```sh
cargo test --lib certificates::tests
cargo test --lib proxy::
```

The disposable local HTTPS ACME service checks ES256 request signatures,
nonces, account identifiers, request URLs, HTTP-01 responses and CSR
signatures. A fake deSEC API and a DNS responder share one record table, so
tests cover a nameserver that answers late, a stale TXT from a crashed run, a
record that never arrives, and a refused write. It issues separate certificates for independent names and an
alias through the real proxy listeners. Tests cover actual TLS handshakes,
live replacement, restart persistence, failure retention, hourly backoff,
expiry refusal and file permissions. No production Host or public DNS is
required for these tests.

Public staging issuance was blocked in this wave. The available remote Host
runs production, and the Operator kept staging blocked. The wave made no
changes to that Host, its traffic or public DNS. Local test certificates do
not prove issuance by a public CA. Run
the staging procedure above on a controlled disposable Host before reporting
that acceptance as passed.
