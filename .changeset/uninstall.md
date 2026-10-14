---
"self-host": patch
---

`self-host uninstall` removes everything the Platform created on the Host, on Linux and macOS: Applications with their data and images, Virtual machines, native Applications and their accounts, the CA in the system trust store, the host DNS routing and the data directory. It lists what goes and asks first, and only touches this Platform's names when Docker is shared. On macOS, `uninstall.sh` runs it and then removes what the installer set up. The installer now marks the `/etc/resolver` file it writes, so the uninstall can tell it from Tailscale's.
