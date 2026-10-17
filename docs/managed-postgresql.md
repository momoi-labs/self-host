# Managed PostgreSQL

Create a private PostgreSQL database from **Databases > New database**.
Version 18 is selected by default; versions 16 and 17 remain available. The Platform generates credentials,
creates a named volume and waits for PostgreSQL to accept connections before
the creation Task succeeds. The Application has no HTTP publication.

## Connect an Application

Open **Connections > Connect application**. Choose an Application and a
Variable name, usually `DATABASE_URL`. The Platform creates a database and a
login for that connection. The login owns its database and has no superuser,
role creation, replication or database creation rights. Other consumer
databases revoke PostgreSQL's default public connection grant.

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
contents. An Application with a live connection
cannot be removed. A Variable owned by a connection cannot be edited through
the generic Variable API. Revoked databases remain in the retained volume;
creating another connection creates another database.

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

This slice does not upgrade PostgreSQL's major version, manage replicas or
create scheduled backups.

## Logical import

Use the database's Imports tab, **New import**, to upload a custom-format
archive of at most 64 MiB:

```sh
pg_dump --format=custom --no-owner --no-privileges --file backup.dump \
  "postgresql://user:password@host:5432/database"
```

The import goes into a connected Application's database, as that
connection's role. Nothing else holds it back: the consumer Application may
keep running, the target may already hold data, and the archive may come from
any PostgreSQL version. pg_restore decides what it can restore.

The restore runs:

```sh
docker exec --user postgres --interactive <database container> \
  pg_restore --verbose --no-password --format=custom --no-owner \
  --no-privileges --single-transaction --exit-on-error \
  --dbname <consumer database> --username <consumer role>
```

The role is not a superuser, so an archive that needs an extension only a
superuser can create fails. A failure rolls the whole restore back. Nothing
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
| `POST /databases` | `{ "name": "database", "major": 18 }` |
| `GET /databases/{id}` | Version, volume, readiness and connection metadata |
| `POST /databases/{id}/redeploy` | Recreate the container and check readiness |
| `POST /databases/{id}/connections` | `{ "consumer_application_id": "...", "variable": "DATABASE_URL" }` |
| `POST /databases/{id}/connections/{connection}/reveal` | Private URL for a ready connection |
| `DELETE /databases/{id}/connections/{connection}` | Revoke access and preserve the consumer database |
| `POST /databases/{id}/connections/{connection}/import` | Custom archive bytes, `application/octet-stream`; `X-Upload-Name` carries the percent-encoded file name |
| `GET /databases/{id}/imports` | None; answers the last 50 imports, newest first, each with its Task |
| `DELETE /databases/{id}` | `{ "confirm_name": "database", "delete_data": false }` |

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
