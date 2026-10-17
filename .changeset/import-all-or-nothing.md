---
"self-host": minor
---

New import has an All or nothing choice, on by default. Off, pg_restore or psql goes on past errors and keeps what restored, and the stage log lists every error at the end. The import still shows as failed, with the number of failed statements and the last error.
