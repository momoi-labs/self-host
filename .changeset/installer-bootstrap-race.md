---
"self-host": patch
---

Upgrading a macOS Host with `install.sh` no longer stops with `Bootstrap failed: 5` and needs a second run. The installer now waits for launchd to let go of a daemon it unloaded before loading it again, and retries the load a few times before giving up.
