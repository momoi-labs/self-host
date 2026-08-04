# Platform state in PostgreSQL 18

The daemon must persist Applications, hostnames, and config beyond the process lifetime. We considered flat files, SQLite, and “no persistence”. We chose **PostgreSQL 18** as an Infra container on Bootstrap: heavier than SQLite for a single-Host MVP, but matches Operator familiarity and avoids an early migration if the data model grows. The Platform starts PG; the Operator does not administer an external database SaaS.

**Status:** accepted
