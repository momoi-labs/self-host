# CLI `self-host`: apps + init --dns

CLI UX follows Disco’s spirit with spaced subcommands. The Application resource in the CLI is **`apps`** (not `projects`), aligned with the glossary. Bootstrap: `self-host init [--dns <suffix>]` with default **`home.lan`** (not `.local`, to avoid mDNS conflicts). One suffix in MVP; multiple later.

**Status:** accepted
