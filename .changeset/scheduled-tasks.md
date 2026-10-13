---
"self-host": patch
---

Settings has a Scheduled tasks pane: work the Platform runs on its own. The first is **Remove unused data**, which runs `prune`. It is off until turned on, then runs daily or weekly on Sunday at the hour picked in the browser's time. Each run is a task with its outcome in Events, a run missed while the Host was off runs once on start, and runs never overlap. **Run now** starts one at once.
