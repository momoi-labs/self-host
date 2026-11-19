---
"self-host": minor
---

The installer asks whether to use a public DNS zone on deSEC for trusted
certificates. The answer defaults to no. Scripted installs set
`SELF_HOST_PUBLIC_ZONE`, `SELF_HOST_ACME_EMAIL` and
`SELF_HOST_DESEC_TOKEN_FILE` instead.
