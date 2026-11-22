---
"self-host": minor
---

The Platform can front a service that already runs on the LAN, such as a
UniFi gateway, as an External Application. Deploy it with the `external`
runtime in the API, `self-host apps add --upstream` or the MCP `deploy_app`
tool. The upstream must be a private LAN address. A self-signed certificate
is accepted only when its SHA-256 fingerprint is pinned.
