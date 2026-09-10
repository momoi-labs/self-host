# self-host

Publish Applications on your own LAN. One binary on the Host serves DNS, HTTP
and HTTPS, and owns a private CA, so every Application you deploy answers at
its own name, `https://<name>.home.lan`, with HTTPS the machines in the house
trust. Docker runs the Applications and nothing else. There is no control
plane anywhere else.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/momoi-labs/self-host/main/install.sh | bash -s -- v0.2.0-beta.2
```

Name the version. This README describes the `v0.2.0-beta.2` pre-release; the
plain `latest` the installer resolves on its own is still `v0.1.0` from August,
which predates Compose Applications, the console and the macOS daemons.

Supported platforms: macOS (Intel / Apple Silicon) and Linux (amd64 / arm64).

On macOS the installer also prepares a Docker runtime and installs the
Platform as a launchd LaunchDaemon, so it starts at boot without a login.
That part is the reason the release is a beta: nobody has yet watched a real
Mac come back from a reboot on its own. [docs/macos-host.md](docs/macos-host.md)
lists the prerequisites and holds the record to fill in.

## Quick start

On Linux, or with `SELF_HOST_BINARY_ONLY=1`:

```bash
self-host init
self-host serve # keep running; use another terminal for the next command
self-host setup-dns # Linux with systemd-resolved
```

For development, choose a fixed API key during initialization:

```bash
self-host init --dns prototype.lan --api-key local
```

Without `--api-key`, `init` generates a random key. The key is saved for the
console and CLI. Running `init` again accepts the same key or no key; it rejects
a different key without replacing the saved one.

DNS starts with `serve`, before Docker is available. It answers
port 53 over UDP and TCP — on Linux at the saved Host IP, on macOS at every
interface, because that is the only privileged bind the Operator gets there
([ADR-0017](docs/adr/0017-host-native-dns.md)). Names under the DNS Suffix
resolve locally; other names go to Cloudflare (`1.1.1.1`, `1.0.0.1`). The
configuration is in `~/.config/self-host/dns.json`; restart `serve` after
changing it. Keep the Host IP fixed.

The Linux installer grants the binary `CAP_NET_BIND_SERVICE`. For a binary
built from source, run `sudo setcap cap_net_bind_service=+ep /path/to/self-host`
after each rebuild. If you supervise the Platform with a systemd system unit
running as the Operator, add `AmbientCapabilities=CAP_NET_BIND_SERVICE` under
`[Service]` instead. The installer still does not configure Linux supervision.

On Linux with systemd-resolved, `self-host setup-dns` reads the saved Host IP
and DNS Suffix and requests administrator privileges to install
`self-host-dns.service`. It routes only the Platform's DNS Suffix to the Host.
The service restores that configuration at boot and when systemd-resolved
restarts. It does not depend on the binary or worktree path. Run the command
again to repair the configuration. `serve` warns if Host DNS does not resolve.
Other Linux resolvers still require manual configuration.

Stop the Platform daemon before resetting or uninstalling. Remove the Host
DNS configuration with:

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

## Platform state

Everything you configure lives in `~/.config/self-host/state/`, written by the
daemon as files it owns. There is no database to run: the console, the API and
your saved configuration work on a Host where Docker is missing or stopped.
Docker runs Applications, and for now serves public HTTPS.

Backing it up, restoring it, and upgrading from an installation that used
PostgreSQL are in [docs/operating.md](docs/operating.md).

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
curl -fsSL https://raw.githubusercontent.com/momoi-labs/self-host/main/install.sh | bash -s -- v0.2.0-beta.2
self-host trust-ca --from admin.home.lan --fingerprint <sha256-from-host>
```

Obtain the fingerprint printed by `self-host init` through a trusted channel.
The command downloads only `/ca.pem` and refuses a mismatch before changing the
Consumer's trust store. Fully restart browsers that were open during installation.
Never copy `ca-key.pem` or `key.pem` to another device.

The Host and Consumers must resolve the DNS Suffix through the Platform DNS.
`self-host init` prints the exact Host command and the DNS address to configure
on the router or each Consumer.

See the [product vision](docs/vision.md) and [next MVP plan](docs/mvp-plan.md)
for the approved direction, and [CONTEXT.md](CONTEXT.md) for domain language.
The plan describes upcoming work, not capabilities already available.

**Docs, issues, and PRs are English-only.**
