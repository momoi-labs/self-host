# Public certificates

The Platform issues one HTTP-01 certificate for every Hostname outside the
DNS Suffix: an Application's Hostname, its aliases and the Hostnames of its
route rules. Names under the Suffix keep the Platform's own CA. The ACME
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
| `GET /certificates/acme` | `{"directory_url", "email"}`, or `null` while issuance is off. |
| `PUT /certificates/acme` with that body | Saves the account. The directory must be `https://`. |
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
addresses, duplicates and paths are refused.

## State and renewal

The authoritative `state/certificates` directory holds:

| File | Contents |
| --- | --- |
| `public.json` | CA directory, contact and managed Hostnames |
| `account.json` | CA directory and serialized ACME account credentials |
| `<hostname>.json` | One certificate chain and its matching private key |
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
signatures. It issues separate certificates for independent names and an
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
