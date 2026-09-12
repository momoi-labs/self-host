# Development environments use Incus system containers

Development environments need a persistent Linux workspace, native tools and
SSH access that survives a Platform restart. Use Ubuntu system containers on
Incus, hosted by Colima on macOS and directly by Linux. This shares the Linux
kernel while allowing systemd to manage the environment's native processes.

Each environment owns its writable disk and a saved tool recipe. Reuse the
development image recipe fields, but apply them through mise inside the existing
environment. Saving a recipe and applying it are separate actions. Do not
publish a personal workspace as a reusable image: that would copy repositories
and credentials along with the tools. Existing development Applications do not
migrate automatically.

This adds a separate Operator workspace resource. It does not replace the
Compose Application model in ADR-0014 or its deferred native Application
supervisor. A full VM per environment would provide a separate kernel, at the
cost of nested virtualization requirements on macOS and more guest overhead.
The Operator accepted the shared kernel boundary for this prototype.

Incus proxy devices provide SSH and web access independently of the Platform.
The Operator reaches them through the Host and an SSH tunnel over the existing
VPN. The Host, Colima on macOS, and Incus must remain running. Automatic VPN
enrollment, public DNS and migration between Hosts are outside this prototype.

The console calls this resource "Virtual machine", as requested by the Operator.
This is the product name. The current runtime still uses Incus system containers
and shares the Linux kernel; renaming does not introduce hardware virtualization.
