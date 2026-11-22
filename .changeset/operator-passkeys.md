---
"self-host": minor
---

The Platform has an Operator account that logs in with passkeys at
`console.{dns_suffix}`. The Platform key registers the first passkey, and
`self-host account recover` on the Host prints a one-time setup key when every
passkey is lost. Creating a Person key, or a key with the `host` scope, now
needs a logged-in session and a fresh passkey confirmation. An API key alone,
the Platform key included, can no longer create one. The console screens for
this come in a later release, so until then such keys cannot be created from
the console. Existing keys keep working.
