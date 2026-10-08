---
"self-host": patch
---

Deleting a native Application on macOS now completes. macOS only deletes an
account when the responsible process has Full Disk Access, which the Platform
daemon cannot hold durably, so the Platform retires the account instead: the
service is removed, nothing can run as the account, its number is never
reused, and the retained data stays protected. `uninstall.sh` still deletes
live and retired accounts from a terminal that macOS can ask (#168).
