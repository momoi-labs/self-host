# Platform State lives in one SQLite file the Platform owns

The Platform keeps its authoritative state in `~/.config/self-host/state/platform.db`,
a SQLite database opened in-process through `rusqlite` with the bundled
engine. There is no server, no container and no connection to manage: the
daemon opens a file, the way it opened `platform.json`.

ADR-0018 moved state out of PostgreSQL because the database ran in a container
and made Docker a dependency of reading configuration. That argument was
against infra, not against a library, and it still holds. What replaced it
worked for Applications, one typed file per record, but every collection added
since (the Zone's Records, Virtual machines, custom images, the audit history)
was stored as a JSON string inside the scalar key-value map of `platform.json`.
Each module parsed its own string, each write rewrote the whole file with the
credentials in it, and the audit history rewrote itself on every event and
grew without bound. The console could not ask "does machine X still exist"
without parsing a `Value`.

## One transaction is the commit unit

Every mutation is one SQLite transaction. Nothing the Platform owns spans two
commit units, which is what ADR-0018 asked of files and what a database gives
for free. The Operator's Compose definition is the one thing still on disk,
verbatim, under a name derived from its digest and written before the row that
references it. A Compose file nothing references is collected at startup, as
before.

The schema is small on purpose:

- `settings`: the scalars (`api_key`, `dns_suffix`, `host_ip`, ...).
- `api_keys`: id, label, creation time.
- `records`: `kind`, `id`, `body`, `updated_at`. One table for every
  collection, including Applications. `body` is the serde struct as JSON, so a
  struct change is a struct change and not a column change. The columns
  pulled out are the ones queries key on; `json_extract` answers the rest,
  and the Application name is checked with it at the commit, where ADR-0018
  wanted uniqueness enforced.
- `audit_events`: the event as JSON plus the columns the list orders and
  prunes by.

`Collection<T>` in `src/collection.rs` is the typed face of `records`. A module
names its kind and gets `Vec<T>`; Rust iterators are the query language until a
filter needs more.

## Settings the daemon relies on

`journal_mode = DELETE`, not WAL, so copying the directory with the daemon
stopped is still a valid backup: one file, no side journal to catch. `synchronous
= FULL` and `fullfsync = ON` make a commit survive a power cut, on macOS too,
where a plain `fsync` is acknowledged before the drive has the data. The file is
`0600`; the journal SQLite writes beside it inherits that mode.

## One writer, as before

The daemon still holds the advisory `flock` on `state/platform.lock`. SQLite's
own locking could refuse a second writer, but the read-only handle that
`self-host init` opens while the daemon runs has to keep working, and the
flock is what already draws that line.

## Schema migrations

`schema_version` holds one number. `src/schema.rs` holds an append-only list of
migrations; index `i` takes a database from version `i` to `i + 1` in its own
transaction. A file that declares a newer version than the binary knows is
refused with the path and both numbers, never guessed at, as ADR-0018 did for
`format`. A shipped migration is never edited; a mistake gets a new one.

Most changes need no migration: a new optional field on a serde struct reads
old bodies with `#[serde(default)]`, and that is the default choice. A
migration is for a new table, column or index, a field old rows carry under
another name, or data computed once for existing rows.

## Audit history keeps thirty days

Every write to the audit history deletes events last updated more than the
retention ago. The default is 30 days: the history is for diagnosis, not
compliance. The Operator changes it on the console's Settings page, which
writes a row in `settings`; `self-host serve --audit-events-max-age` pins it from
the command line. Precedence is PostgreSQL's, reduced to the levels that
exist: default, then the Operator's setting (`ALTER SYSTEM`), then the command
line (`postgres -c`). A setting written while the flag pins the value is kept
and inert, and applies again once the flag is gone. The API and the page always
say which level produced the effective value.

## Upgrading from the JSON files

No installation runs in production, so the import is a script, not daemon
code. A daemon that finds `platform.json` next to `platform.db` stops and
names `scripts/import-json-state.py`; it never reads the JSON and never starts
an empty installation beside it. The script runs with the daemon stopped,
imports in one transaction, and moves the JSON files to
`state/imported-<date>/` rather than deleting them. The script and the guard
leave together once the known installations have moved. See
[operating the Platform](../operating.md).

**Status:** accepted

**Amends:** [ADR-0018](0018-platform-state-in-files.md). Files the Platform
owns, one writer, `0600`, refusing a newer format: all kept. The commit unit
is a transaction rather than a file, and the collections stop living inside
the settings map.
