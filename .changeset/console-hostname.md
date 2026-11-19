---
"self-host": minor
---

The console answers on `console.<suffix>` on every new Host. `admin.<suffix>`
stays as a legacy alias with the same answer, certificate and routing, and an
Application still cannot take either name. The CLI, the installer and the
trust-ca instructions use `console.<suffix>`.
