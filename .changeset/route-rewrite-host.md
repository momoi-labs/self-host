---
"self-host": minor
---

The proxy can send an Application's Web Target address as `Host` instead of the Hostname a client asked for. Turn it on for every Application under Settings > General, or set `rewriteHost` on `PUT /settings`. Each Application can override it on its Routes tab, or with `rewrite_host` on `PUT /apps/id/{id}/routes`. A change applies at once, without a rebuild or restart, and `X-Forwarded-Host` still names the Hostname. Use it for an app that refuses any `Host` but a loopback one, such as `laya-apple serve`.
