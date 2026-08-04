# LAN exposure: app HTTP, DNS, Operator API + API key

On the Host, the Platform publishes LAN port **80** (Traefik, Consumer traffic) and **53** (dnsmasq). Applications do not publish host ports directly — traffic enters by Hostname. The **Operator HTTP API** also listens on the LAN so the CLI (and a future web console) can run on another device. Auth: **API key** generated on `init`, stored in local CLI config (Disco-style). mTLS and arbitrary Application port publish are out of MVP.

The “API = gRPC” binding is **superseded by ADR-0007**; Operator API exposure on the LAN remains.

**Status:** accepted
