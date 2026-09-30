---
"self-host": minor
---

An Application's record now says how it runs and whether it is published. A
Compose Application can be created unpublished: no Hostname, no DNS Record, no
proxy route and no Host port, which is what a worker or a database wants.
Requests and responses gain optional `runtime`, `publication` and
`variable_delivery` fields; existing Applications and older clients keep
working unchanged. A native runtime is recognized but refused with 501 until a
later release runs it.

The Platform resolves Compose files itself: `${VAR}` and the other Compose
forms read the Application's Variables and never the daemon's environment, and
a Variable reaches a service only where the file names it. Applications
created before this keep the old behavior, every Variable on every service,
until switched with `variable_delivery`. YAML merge keys and `x-` extension
fields are accepted; unsupported volume options and mount modes are refused by
name.
