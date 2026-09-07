# self-host

Super easy home lab PaaS: publish apps on your LAN Host without an external control-plane SaaS.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/momoi-labs/self-host/main/install.sh | bash
```

Supported platforms: macOS (Intel / Apple Silicon) and Linux (amd64 / arm64).

On macOS the installer also prepares a Docker runtime and installs the
Platform as a launchd LaunchDaemon, so it starts at boot without a login.
See [docs/macos-host.md](docs/macos-host.md) for prerequisites.

## Quick start

On Linux, or with `SELF_HOST_BINARY_ONLY=1`:

```bash
self-host init
self-host setup-dns # Linux with systemd-resolved
self-host serve
```

On Linux with systemd-resolved, `self-host setup-dns` reads the saved Host IP
and DNS Suffix and requests administrator privileges to install
`self-host-dns.service`. It routes only the Platform's DNS Suffix to the Host.
The service restores that configuration at boot and when systemd-resolved
restarts. It does not depend on the binary or worktree path. Run the command
again to repair the configuration. `serve` warns if Host DNS does not resolve.
Other Linux resolvers still require manual configuration.

To remove the Host DNS configuration before resetting or uninstalling:

```bash
sudo systemctl disable --now self-host-dns.service
sudo rm /etc/systemd/system/self-host-dns.service
sudo systemctl daemon-reload
```

Then visit `https://admin.<your-dns-suffix>` (default: `https://admin.home.lan`).

Deploy a container image, or a Compose file:

```bash
self-host apps add --name blog --image nginx:alpine
self-host apps add --name hermes --compose-file hermes.yml --web-port 9119
self-host apps stop hermes && self-host apps start hermes
```

The supported Compose subset is in
[docs/compose-applications.md](docs/compose-applications.md); the Hermes
workflow is in [docs/hermes.md](docs/hermes.md).

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
