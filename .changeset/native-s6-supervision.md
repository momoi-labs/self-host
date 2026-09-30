---
"self-host": minor
---

Add Linux s6 supervision for native processes using the existing dedicated
non-root launcher. Supervision supports readiness checks, crash recovery,
descendant cleanup, rotating logs and persisted running or stopped intent.
The installer can opt into one boot supervisor with cgroup delegation.
Native Application API requests remain disabled until lifecycle integration.
