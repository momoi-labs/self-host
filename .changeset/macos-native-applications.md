---
"self-host": minor
---

Run native Applications on a macOS Host. Each Application runs under its own
non-administrator account, supervised by s6. A protected helper, reached
through a restricted sudo rule, creates the accounts and controls the
supervisor, so the Platform keeps running as the Operator. The macOS release
bundles s6, and the installer sets up the helper and a boot LaunchDaemon.

On macOS the console hides resource limits, metrics, the terminal and managed
PostgreSQL connections for native Applications, and the API rejects them.

Deleting a native Application on macOS stops it and protects its data, but
cannot remove its account yet: macOS requires Full Disk Access for that (#168).
