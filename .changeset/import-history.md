---
"self-host": minor
---

A database's Imports tab lists every import into it, newest first, with its Application, file, size, result and imported object count, and shows the steps and pg_restore log of the one you select. The import runs in the daemon, so the list shows how it ended even if the console was closed. `GET /databases/{id}/imports` returns the same list. A database keeps its last 50 imports.
