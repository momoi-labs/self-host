---
"self-host": minor
---

The Platform has an Operator account that logs in with passkeys at
`console.{dns_suffix}`. The Platform key registers the first passkey, and
`self-host account recover` on the Host prints a one-time setup key when every
passkey is lost. Creating a Person key, or a key with the `host` scope, now
needs a logged-in session and a fresh passkey confirmation. An API key alone,
the Platform key included, can no longer create one. Existing keys keep
working.

The console logs in with passkeys only and keeps its session in a cookie. The
first passkey login retires the Platform key for good, even after a restart
with `SELF_HOST_API_KEY` set. The console then offers a Person key for the CLI,
which `self-host account use-key` saves. Settings, Access, Passkeys lists, adds
and removes passkeys, and each key shows the passkey that authorized it.
