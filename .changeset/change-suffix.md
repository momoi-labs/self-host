---
"self-host": minor
---

`self-host change-suffix <new>` moves an initialized Host to a new DNS Suffix
while the Platform is stopped. It copies the state first, then rewrites
Application and machine names, the DNS config, the CLI config and the local
certificate. `--dry-run` prints every change and writes nothing.
