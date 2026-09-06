# self-host

Super easy home lab PaaS: publish apps on your LAN Host without an external control-plane SaaS.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/momoi-labs/self-host/main/install.sh | bash
```

Supported platforms: macOS (Intel / Apple Silicon) and Linux (amd64 / arm64).

## Quick start

```bash
self-host init
self-host serve
```

Then visit `https://admin.<your-dns-suffix>` (default: `https://admin.home.lan`).

## Local HTTPS

Bootstrap creates one private CA and a wildcard certificate for the chosen DNS
Suffix. The CA stays unchanged until `self-host reset` removes the Platform.

Trust the CA on the Host with:

```bash
self-host trust-ca
```

On another machine, first configure its DNS, then install the public CA directly
from the Host:

```bash
curl -fsSL https://raw.githubusercontent.com/momoi-labs/self-host/main/install.sh | bash
self-host trust-ca --from admin.home.lan --fingerprint <sha256-from-host>
```

Obtain the fingerprint printed by `self-host init` through a trusted channel.
The command downloads only `/ca.pem` and refuses a mismatch before changing the
Consumer's trust store. Fully restart browsers that were open during installation.
Never copy `ca-key.pem` or `key.pem` to another device.

The Host and Consumers must resolve the DNS Suffix through the Platform DNS.
`self-host init` prints the exact Host command and the DNS address to configure
on the router or each Consumer.

On a Linux Host with UFW enabled, Bootstrap also prints scoped rules that let
Traefik reach the Operator API from its two Docker networks.

See the [product vision](docs/vision.md) and [next MVP plan](docs/mvp-plan.md)
for the approved direction, and [CONTEXT.md](CONTEXT.md) for domain language.
The plan describes upcoming work, not capabilities already available.

**Docs, issues, and PRs are English-only.**
