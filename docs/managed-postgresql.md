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

Stop the consumer Application, then use the database's Import tab to upload a
`pg_dump --format=custom` archive of at most 64 MiB. The Platform checks the
source server and dump-tool major versions against the selected target major.
It refuses newer archives and nonempty target databases.

The restore runs as the consumer's database role, with `--no-owner`,
`--no-privileges`, `--single-transaction` and `--exit-on-error`. Dumps needing
privileged extensions are refused. A SQL failure rolls back the transaction.
After success, the Task checks the database's non-system relations and records
the object count. Query application-specific data before using the result.
These options follow PostgreSQL's [pg_restore documentation](https://www.postgresql.org/docs/18/app-pgrestore.html).

Import occupies the consumer Task queue and the provider operation lock.
Consumer start and provider stop, restart, disconnect or removal cannot run
through an active import. SQL has statement and lock timeouts. The container
also bounds each PostgreSQL client process to 14 minutes, with a five-second
termination grace period; the Host waits at most 15 minutes for Docker.

An interrupted client or lost Docker connection does not prove whether the
last transaction committed. The Task reports failure without claiming rollback.
Check the target before retrying. The empty-database guard prevents overwriting
an import that committed. Private upload files are removed when the import
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
| `POST /databases/{id}/connections/{connection}/import` | Custom archive bytes, `application/octet-stream` |
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
its URL, restart and recreate the database, import into a stopped empty
consumer, disconnect, then verify the two data-removal choices. Inspect the
Summary, Last operation and Logs tabs for readiness and failures. Last operation
shows the recorded stages, duration and outcome. Events identify the resource
as a database; older events for existing databases receive the same label.
