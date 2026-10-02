# Public certificates

The Platform issues one HTTP-01 certificate per configured Hostname, including
aliases. It stores the ACME account and private keys in Platform State. An
Application never receives them. Public issuance is disabled until the
Operator creates `state/certificates/public.json`.

## Configure an isolated Host

Use a disposable Host and Hostnames you control. Their public DNS must point
to that Host, and the CA must reach its HTTP listener on port 80. Synthetic
`.invalid` names work only with the local test CA.

1. Stop the disposable Platform daemon.
2. Create `certificates` under its Platform State directory with mode `0700`.
3. Write `public.json` below with mode `0600`. Creating this file authorizes
   account registration and accepts the configured CA's terms of service.
4. Start the daemon. It binds the HTTP challenge listener before requesting
   certificates. It checks for issuance and renewal once a minute.

```json
{
  "directory_url": "https://acme-staging-v02.api.letsencrypt.org/directory",
  "contact": ["mailto:operator@example.invalid"],
  "hostnames": ["blog.example.invalid", "alias.other.invalid"]
}
```

Replace the synthetic names and contact on the disposable Host. Use staging
first. Staging certificates are not trusted by ordinary browsers. A private
ACME test CA can set `ca_cert_path` to its PEM trust anchor. The Platform
verifies the ACME service's HTTPS certificate; it never disables that check.

Changes to `public.json` take effect when the daemon restarts. Hostnames must
be explicit DNS names. Wildcards, IP addresses, duplicates and paths are
refused. Changing the CA directory while retaining its account file fails
closed. Keep separate state for separate CAs.

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
Public certificate controls in the console remain W3.

## TLS selection and challenges

An explicitly managed public Hostname receives only its exact saved
certificate. A missing, expired or not-yet-valid certificate refuses the TLS
handshake. The proxy does not replace it with the LAN wildcard.

Other SNI names receive the LAN certificate only when its actual SANs cover
the name. A wildcard covers one label, so `*.home.lan` covers `app.home.lan`
and does not cover `deep.app.home.lan`. Unknown independent names refuse the
handshake. Clients without SNI retain the valid LAN certificate for setup
compatibility. The HTTP Hostname check still controls access to management
and Application routes.

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
