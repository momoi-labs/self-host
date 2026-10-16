---
"self-host": patch
---

A database import no longer waits for the consumer Application to be stopped, so an Application that failed its first deploy can take its data. The restore now shows pg_restore's log, error included, under Last operation, and the Import tab gives the `pg_dump` command that makes a compatible archive. Stopping a Compose Application whose definition needs a Variable that is not set, such as a Git Application whose first deploy failed for it, stops it instead of failing with "The database runtime operation failed". Every database stage in Last operation now lists the commands it ran with their output and exit status, passwords cut out, instead of "Check PostgreSQL and Docker logs", and an archive from a newer PostgreSQL names both versions.
