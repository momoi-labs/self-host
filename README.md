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

See `docs/mvp-scope.md` for the confirmed MVP and [`CONTEXT.md`](./CONTEXT.md) for domain language.

**Docs, issues, and PRs are English-only.**
