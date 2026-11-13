# Managed PostgreSQL

Create a private PostgreSQL database from **Databases > New database**.
Version 18 is selected by default; versions 16 and 17 remain available. The Platform generates credentials,
creates a named volume and waits for PostgreSQL to accept connections before
the creation Task succeeds. The Application has no HTTP publication.

## Connect an Application

Open **Connections > Connect application**. Choose an Application, a
Variable name, usually `DATABASE_URL`, and a **Database**. Every connection
gets its own login, with no superuser, role creation, replication or database
creation rights. The Database choice decides what that login reaches:

- **New database** (the default): the Platform creates a database named after
  the Application, for example `mochi_cut` for `mochi-cut`. The name can be
  edited. It uses lowercase letters, digits and underscores, starts with a
  letter or underscore, has at most 63 characters, and cannot start with
  `pg_` or be `postgres`, `template0` or `template1`. A name another database
  on the server already has is refused. The connection's login owns the new
  database, and the database revokes PostgreSQL's default public connection
  grant.
- **Existing database**: one the Platform made on this server, by an earlier
  connection or by an import. The list shows which Applications use each one.
  The database is owned by a role that cannot log in, and every login on it is
  a member whose sessions there act as that owner. The first time a database
  is shared, its own login hands it and its objects to such an owner role and
  joins it too; that login's open sessions to the database end once, and
  reconnect as the owner. The Application reads and writes the data already
  there, and what it creates the other Applications on the database can use
  too. No login is a member of another, so one Application cannot take over
  another's login. A database the Platform did not make, such as `postgres`, is
  not offered.

Two Applications can connect to the same database this way. The API keeps a
generated name (`db_<token>`) when a request names no database.

A container consumer joins the provider's private network. Its URL names the
database container. A Native Application receives a loopback URL only after
the Platform verifies its dedicated Application Account. The database publishes
that transport port only on `127.0.0.1`. Host processes can reach loopback, so
the generated database credentials remain the access control.

The consumer's own Task queue applies the Variable and recreates its runtime
when needed. A stopped Application stays stopped. A Compose consumer must
reference the Variable in its definition. The provider's Variables are never
broadcast to consumers. Connection status reaches `ready` only after the
consumer Task finishes.

Connection metadata contains the database name, role, Variable name and Task
status. The Connections table lists each consumer. **View > Reveal URL**
requests the URL through an authenticated endpoint with `Cache-Control: no-store`.

**Disconnect** disables the role, clears its password, terminates its sessions
and removes the managed Variable. The connection becomes `revoked` only after
PostgreSQL confirms revocation and consumer cleanup completes. A stopped or
unreachable provider leaves it `revoking`, with deletion and Variable ownership
guards still active. Disconnect preserves the consumer database and its
contents, and only disables that connection's login: other Applications on the
same database keep their access. An Application with a live connection
cannot be removed. A Variable owned by a connection cannot be edited through
the generic Variable API. A disconnected database stays on the server and is
offered again under **Existing database**.

## Extensions and custom images

The official image has the extensions PostgreSQL ships with. For any other
extension, run the server from a custom image:

1. Under **Custom images**, choose **New image** and the **PostgreSQL**
   template. The template is a Dockerfile: the official image plus pgBackRest,
   with a commented example of an extension package.
2. Set the version in `FROM` to the major version of the server. Add the
   extension's package, for example `postgresql-17-pgvector`. Then **Save and
   build**.
3. Under **New database**, pick the image in the **Image** field. From the
   CLI: `self-host db create --name main --major 17 --image <image name>`.

The Platform refuses an image that does not run the chosen major version. The
image is chosen at creation and stays with the server.

To add an extension later, edit the same custom image and save it. When the
build is ready, press **Recreate** on the server. Recreate moves the server to
the image's current build and keeps the volume. A custom image a server uses
cannot be deleted.

Backups need pgBackRest in the image. The template installs it. If you remove
it, or write the Dockerfile yourself without it, turning backups on fails and
names the lines to add.

## Persistence and removal

Restart and Recreate preserve the volume. Recreate replaces the container
using the same selected major version. Ordinary Application configuration
cannot replace the managed image, Compose definition or credentials.

PostgreSQL 16 and 17 keep their named volume at `/var/lib/postgresql/data`.
PostgreSQL 18 mounts it at `/var/lib/postgresql`, with the cluster under
`/var/lib/postgresql/18/docker`, following the
[official image's storage layout](https://hub.docker.com/_/postgres#pgdata).
Existing 16 and 17 volumes keep their original layout and major version.

Remove database requires typing its Application name and disconnecting every
consumer. By default the Platform removes the container and retains the volume.
The dialog has a separate **Permanently delete the stored database data**
checkbox. Only that explicit choice removes the volume. A retained volume is
not adopted automatically by a new database Application.

This slice does not upgrade PostgreSQL's major version or manage replicas.

## Backups

A server with backups on keeps a pgBackRest repository in an S3-compatible
bucket: physical backups of the whole server and every WAL segment since
([ADR-0035](adr/0035-postgresql-backups-are-pgbackrest-repositories-with-point-in-time-recovery.md)).
A restore makes a new server, at the latest state or at a time you pick.

Each server has its own repository at `<prefix>/<database id>/`. Backups are
not encrypted by self-host. Use a private bucket, and turn on the provider's
encryption at rest when it has one.

### Set up

1. In **Settings**, fill **Backup storage**: an HTTPS endpoint (empty for AWS
   S3), region, bucket, optional prefix and the access keys. The bucket must
   exist. Clear **Verify the TLS certificate** only for a bucket on your own
   network with a self-signed certificate. **Test connection** writes, reads
   and deletes a small object. A Host has one backup storage. The secret key
   stays in private Platform State and is never shown again. While any
   server has backups on, or is turning them on or off, you can change the
   keys but not the endpoint, bucket or prefix.
2. In **Settings**, set the **Maintenance window**: the hour it starts, in the
   Host's local time, and how long it lasts. The default is 02:00 for 4 hours.
3. On the server's **Backups** tab, check **Back up this server**, set the
   days to keep, then **Save**.

Turning backups on runs as a Task. It builds the custom image
`postgres-<major>-backups` (the official image plus pgBackRest), recreates the
container with it, creates the repository, turns WAL archiving on and takes
the first full backup. PostgreSQL restarts once. Turning backups off restarts
it once more and puts the official image back. The repository stays in the
bucket.

Custom images lists `postgres-<major>-backups` with a **Platform** badge. The
Platform writes and rebuilds it, so the console shows it read-only and the API
answers 409 to an edit.

A server created from a custom image keeps that image with backups on and
off. The Task checks that the image has pgBackRest and builds nothing. A
server restored from its backups runs the same custom image.

### Schedule and retention

| Day | Backup |
| --- | --- |
| Sunday | Full |
| Wednesday and Saturday | Differential |
| Other days | Incremental |

A scheduled backup starts when the maintenance window opens. A Host that was
off at that hour runs it when it comes back, if the window is still open.
Otherwise it waits for the next window. **Back up now** takes an incremental
backup at any time.

PostgreSQL sends each finished WAL segment to the bucket, and closes a segment
at least once a minute while there are writes. At most one minute of writes is
outside the bucket.

**Days to keep** is how far back a restore can go. After each backup,
pgBackRest deletes the full backups and the WAL that are older than that. You
do not delete single backups.

### Restore

A restore makes a **new server** and never touches the source. On the Backups
tab, choose **Restore to a new server**, name it, and pick the latest state or
a date and time. **Restore from backup** on the Databases list opens the same
dialog and asks which server to restore from. It lists the servers that have a
backup.

The console opens the new server on its Last operation tab, where the restore
runs. The source keeps running. The new server has the databases, roles and
passwords the source had at that moment. It has no connections, and its backups are off.

To put restored data in place of a damaged database, restore, check the new
server, then connect the Application to it from its Connections tab as an
existing database.

A restore that fails leaves the new server in the list with the status
`failed`. Remove it before you try again.

### Verify

**Verify latest backup** restores the latest backup into a throwaway volume
and container with no published port, starts PostgreSQL, queries every
database, then removes both. It needs free disk space for one copy of the
server. The result shows as **Last verification**. Nothing verifies on a
schedule.

### When the bucket does not answer

PostgreSQL keeps the WAL it could not send and tries again. Nothing is lost.
The Backups tab shows **Backups need attention** and the list of databases
shows **Problem** on the server. The WAL grows on the data volume until the
bucket answers again, so a long outage can fill the disk.

### Limits

- One bucket per Host, over HTTPS. No local-disk or SFTP target.
- A restore is the whole server, on the same PostgreSQL major. To move one
  database or change major, use an import.
- No restore over the source server in place.
- Removing a server forgets its backup settings. Its repository stays in the
  bucket.
- Backups run inside the database container and stream to the bucket. They
  need no extra disk on the Host.

### CLI

```bash
self-host db backup storage set --endpoint https://s3.example.com \
  --region us-east-1 --bucket backups --prefix home \
  --access-key-id KEY --secret-access-key-file ./secret
self-host db backup storage test
self-host db backup window set --start 2 --hours 4
self-host db backup enable main --keep-days 14
self-host db backup now main --type full
self-host db backup list main
self-host db backup verify main
self-host db backup restore main --as main-restored
self-host db backup restore main --as main-monday --time 2026-11-09T14:30:00Z
self-host db backup disable main
```

`storage set` takes `--no-verify-tls` for a self-signed certificate.
Without `--secret-access-key-file`, the CLI reads the secret from
`SELF_HOST_S3_SECRET_ACCESS_KEY` or asks for it without echo.

## Logical import

Use the database's Imports tab, **New import**. The dump comes from one of two
places:

- **Upload a file:** the browser sends it, and the daemon writes it to private
  storage (`state/postgres-imports`, readable by the daemon only) as it
  arrives, then removes it when the import ends. There is no size limit. The
  upload runs in the background: the New import dialog closes at once, the
  Imports list shows its progress, and it goes on while you use other console
  screens. Refreshing or closing the page stops it, and the console asks
  first. The file goes in 8 MiB chunks, and the daemon keeps every byte that
  arrived. **Retry** in the list sends only the rest. After a refresh, choose
  the same file again in New import and the upload goes on from the bytes
  the Host holds; the list shows its progress from there. Another file
  starts over. A file counts as the same when its name, size, modification
  time and the hash of its first and last MiB match. An upload nobody sent a
  byte to for a day is removed, file included. **Dismiss** removes it at once.
- **Path on the Host:** a file, or a directory for a directory dump, that
  already sits on the Host that runs self-host. The daemon reads it where it
  is and never moves or removes it.

Any pg_dump format imports:

| Type | Made with | Restored with |
| --- | --- | --- |
| Custom | `pg_dump --format=custom` | `pg_restore --format=custom` |
| Tar | `pg_dump --format=tar` | `pg_restore --format=tar` |
| Directory | `pg_dump --format=directory` | `pg_restore --format=directory`, Host path only |
| Plain SQL | `pg_dump --format=plain` | `psql --file` |

```sh
pg_dump --format=custom --no-owner --no-privileges --file backup.dump \
  "postgresql://user:password@host:5432/database"
```

The form reads the type from the file and preselects it: a custom archive
starts with `PGDMP`, a tar archive has a tar header, plain SQL is text that
starts like a pg_dump script, and a directory dump holds a `toc.dat`. When it
cannot tell, for example for a compressed file, nothing is preselected and
Import waits for the type to be chosen.

The import goes into a connected Application's database, as that
connection's role, or into a database with no Application connected. For the
latter, choose **A new database** as the consumer database and name it, with
the same rules as above. The import creates it first, with its own login that
owns it, and restores as that login. The Imports tab lists the import with the
database name. Later imports can choose that database again, and an
Application connects to it later as an **Existing database**. Nothing else holds it back: the consumer Application may
keep running, the target may already hold data, and the dump may come from
any PostgreSQL version. pg_restore or psql decides what it can restore.

A file streams into the tool's standard input, so its size never sits in the
daemon's memory. A directory is copied into the database container with
`docker cp`, owned by `postgres`, and removed after. A custom archive runs:

```sh
docker exec --user postgres --interactive <database container> \
  pg_restore --verbose --no-password --format=custom --no-owner \
  --no-privileges --single-transaction --exit-on-error \
  --dbname <consumer database> --username <consumer role>
```

and plain SQL runs `psql --no-psqlrc --no-password --echo-errors --set
ON_ERROR_STOP=1 --host 127.0.0.1 --dbname ... --username ... --file -`, fed
`BEGIN;`, the dump and `COMMIT;`. psql runs in a throwaway copy of the
database image (`docker run --rm --network container:<database>`, as user
65534) and connects over TCP with the connection role's password. A SQL dump
can carry psql meta-commands such as `\!` (a shell) or `\connect`; there a
shell reaches no database files or trusted local socket, and every
connection needs a password. The throwaway container still shares the
database's network. The `COMMIT;` is sent only once the whole dump was read, so a
dump cut short rolls back instead of keeping its first half. Each output
stream keeps its last MiB. A gzip-compressed SQL dump is unpacked
in the container on the way in (`gzip --decompress --stdout | psql ...`); its
type cannot be read from the file, so choose Plain SQL. A Host path is read on
the machine that runs self-host, not the one with the browser.

**All or nothing** is on by default: pg_restore runs with
`--single-transaction --exit-on-error`, and psql with `ON_ERROR_STOP=1`
inside the `BEGIN;` and `COMMIT;`. The first error stops the restore and rolls
all of it back. Turn it off in the New import form, or send
`X-Import-All-Or-Nothing: false` or `"all_or_nothing": false` to the API, and
the restore goes on past errors as pg_restore and psql do without those
options: what works stays, statement by statement. The stage log then ends
with every error the tool printed, and the import still fails, naming how many
statements failed and the last error, so an import with errors never reads as
a clean one. The object count is not recorded for it.

The role is not a superuser, so an archive that needs an extension only a
superuser can create fails. Nothing
times the restore: it runs as long as the archive needs. After success, the
Task counts the database's non-system relations and records the number.
These options follow PostgreSQL's [pg_restore documentation](https://www.postgresql.org/docs/18/app-pgrestore.html).

The restore stage shows that command and everything pg_restore printed, and
a failure names the last error line it printed. The Platform's own passwords
are cut out of both. The Imports tab lists every import into the database,
newest first, with its Application, file, result and imported object count,
and shows the steps and log of the one selected. A database keeps its last 50
imports.

The import is a Task on the database's own queue: an operation on the same
database queued after it waits its turn, and the consumer Application is not
held back. Closing the console does not stop it.

An interrupted client or lost Docker connection does not prove whether the
last transaction committed. The Task reports failure without claiming rollback.
Check the target before retrying, since a retry imports again on top of an
import that may have committed. Private upload files are removed when the import
worker finishes; an abrupt daemon shutdown can leave a private file for manual
cleanup under `state/postgres-imports`.

## API

Every endpoint requires Operator authentication. Mutations return a Task id.

| Endpoint | Request or result |
| --- | --- |
| `POST /databases` | `{ "name": "database", "major": 18 }`; optional `"custom_image": "<custom image id>"` runs that image in place of the official one. It must be built and run the same major version |
| `GET /databases/{id}` | Version, volume, readiness and connection metadata |
| `POST /databases/{id}/redeploy` | Recreate the container and check readiness |
| `POST /databases/{id}/connections` | `{ "consumer_application_id": "...", "variable": "DATABASE_URL", "database": { "new": "mochi_cut" } }`; `{ "existing": "mochi_cut" }` joins a database the Platform made; without `database` the name is generated |
| `GET /databases/{id}/databases` | None; answers the consumer databases the Platform made on the running server, each with `name`, `standalone` (an import made it) and `consumers` |
| `POST /databases/{id}/connections/{connection}/reveal` | Private URL for a ready connection |
| `DELETE /databases/{id}/connections/{connection}` | Revoke access and preserve the consumer database |
| `POST /databases/{id}/connections/{connection}/import` | The whole dump in one request, which cannot resume, as `application/octet-stream`; `X-Upload-Name` carries the percent-encoded file name, `X-Import-Format` the type (`custom`, `tar`, `sql`) when it should not be read from the file; `X-Import-All-Or-Nothing: false` restores past errors |
| `POST /databases/{id}/imports/uploads` | `{ "connection_id": "...", "name": "app.dump", "size": 1048576, "fingerprint": "...", "format": "custom", "all_or_nothing": true }` starts a resumable upload, or finds the one started for the same connection, name, size and fingerprint; answers `{ "id": "...", "size": 1048576, "received": 0 }`. `"database": "name"` in place of `connection_id` uploads into a database with no connection, made when new. `format` and `all_or_nothing` are optional; a resumed upload takes the latest ones |
| `GET /databases/{id}/imports/uploads/{upload}` | None; answers `{ "id", "size", "received" }` |
| `PUT /databases/{id}/imports/uploads/{upload}` | One chunk with `Content-Range: bytes start-end/size`. It must start at `received` and end inside `size`; otherwise 409 answers `received`, or 400. A chunk cut short keeps what arrived. Answers `{ "id", "size", "received" }` |
| `POST /databases/{id}/imports/uploads/{upload}/import` | None; queues the import of a complete upload and answers its Task id. A refusal keeps the upload. Asked again, a queued upload answers the same Task id; it is not resumed by a new start |
| `DELETE /databases/{id}/imports/uploads/{upload}` | None; removes the upload and its bytes |
| `POST /databases/{id}/connections/{connection}/import-path` | `{ "path": "/srv/backups/app.dump", "format": "custom", "all_or_nothing": true }`; `format` and `all_or_nothing` are optional, and `all_or_nothing` defaults to true |
| `POST /databases/{id}/databases/{name}/import` and `.../import-path` | The same, into the database `name` with no connection; the import creates it when it is new. A connection's database is refused |
| `POST /databases/import-format` | `{ "path": "..." }`; answers `{ "format": "custom" }`, or `null` when the type cannot be told |
| `GET /databases/{id}/imports` | None; answers the last 50 imports, newest first, each with its Task |
| `DELETE /databases/{id}` | `{ "confirm_name": "database", "delete_data": false }` |
| `GET /backups/storage` | None; answers `configured`, `endpoint`, `region`, `bucket`, `prefix`, `path_style`, `verify_tls`, `access_key_id` and `has_secret`. Never the secret key |
| `PUT /backups/storage` | `{ "endpoint": "", "region": "us-east-1", "bucket": "backups", "prefix": "home", "path_style": true, "verify_tls": true, "access_key_id": "...", "secret_access_key": "..." }`; without `secret_access_key` the saved one stays. An `http://` endpoint answers 400. A new endpoint, bucket or prefix answers 409 while any server has backups on or is turning them on or off |
| `POST /backups/storage/test` | The same body tests it without saving; no body tests the saved storage. A storage that fails answers 422 |
| `DELETE /backups/storage` | None; 409 while a server has backups on |
| `GET /maintenance-window` and `PUT /maintenance-window` | `{ "start_hour": 2, "hours": 4 }` in the Host's local time; the answer adds `utc_offset` |
| `GET /backups` | None; one row per server with backups on or a repository: `database_application_id`, `enabled`, `last` and `problem` |
| `GET /databases/{id}/backups/settings` and `PUT` | `{ "enabled": true, "keep_days": 14 }`, each optional. The answer adds `next_run_at`, `next_kind`, `storage_configured`, `archiving`, `verification` and `problem`. A `PUT` that changes `enabled` queues a Task and adds its `task_id` |
| `GET /databases/{id}/backups` | None; answers the repository's backups, newest first, each with `label`, `kind` (`full`, `diff` or `incr`), `started_at`, `finished_at`, `bytes` and `repository_bytes` |
| `POST /databases/{id}/backups` | Optional `{ "kind": "full" }`, default `incr`; queues a backup and answers its Task id |
| `POST /databases/{id}/backups/restore` | `{ "name": "main-restored", "time": "2026-11-09T14:30:00Z" }`; `time` is optional. Queues a restore into a new server and answers its `id` and the `task_id` |
| `POST /databases/{id}/backups/verify` | None; queues a test restore of the latest backup |

The Application response adds optional `managed_postgres` metadata. Older
Applications retain their existing shape and behavior. Definition metadata and
connection references use typed Platform State collections; passwords never
enter Task payloads, process arguments or audit responses.

## Disposable acceptance test

On an isolated Linux Host with Docker and the updated daemon running, set
`SELF_HOST_TEST_URL` to its loopback API and `SELF_HOST_TEST_KEY` to its key.
Run `python3 scripts/test-managed-postgres.py`. The script creates synthetic
PostgreSQL 18 fixtures, checks the versioned volume layout, persistence through
restart and recreation, private credentials, import and revocation, then removes
only what it created. It retains failed fixtures for diagnosis.

For backups, run `python3 scripts/test-postgres-backups.py` with the same
environment against a Host that has no backup storage. It needs `openssl`. It
starts its own S3 server in Docker (`SELF_HOST_TEST_S3_IMAGE`, default
`versity/versitygw`) behind a TLS proxy with a self-signed certificate, turns
backups on for a new server, verifies the latest backup and restores to a time
between two rows into a new server. The database containers must reach the
proxy on an address of the Host. Set `SELF_HOST_TEST_S3_ADDRESS` when a
firewall blocks the default one.

With native supervision and `postgresql-client` installed, run
`python3 scripts/test-native-postgres.py` with the same environment. It checks
the dedicated non-root identity, loopback transport, restricted database
login, stopped consumer behavior and credential revocation.

For the console, create a database, connect a test Application, reveal and hide
its URL, restart and recreate the database, import into a consumer, disconnect, then verify the two data-removal choices. Inspect the
Summary, Last operation and Logs tabs for readiness and failures. Last operation
shows the recorded stages, duration and outcome, and under each stage the
`docker` commands it ran with their output and exit status. Every password the
Platform made is cut out, and so is any password in a connection URL, after
`PASSWORD` in SQL or in a `*PASSWORD=` variable. Events identify the resource
as a database; older events for existing databases receive the same label.
