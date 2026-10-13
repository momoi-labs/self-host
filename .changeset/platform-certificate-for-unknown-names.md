---
"self-host": minor
---

A hostname without a certificate of its own now answers HTTPS with the Platform's own certificate instead of refusing the handshake. A browser reaching the Host directly still rejects the mismatch, but an edge proxy that trusts the origin on its own terms, such as Cloudflare in "Full" mode, completes the handshake and serves the Application. A Host whose domains all sit behind such a proxy needs no ACME account.
