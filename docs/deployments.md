# Deployment triggers and recovery

Git updates still require manual review by default. An Operator can enable a
scoped deploy trigger for one Application from its Deployments tab. Enabling
a trigger lets that branch deploy without the review dialog. No Application
receives a trigger when it is created.

## Optional automatic deployment

The Operator API uses the existing API key:

| Request | Result |
| --- | --- |
| `GET /apps/id/{id}/deploy-trigger` | Whether a trigger is enabled, its branch and creation time. No token. |
| `POST /apps/id/{id}/deploy-trigger` with `{"branch":"main"}` | Enable or rotate the token for the configured branch. The token is shown once. |
| `DELETE /apps/id/{id}/deploy-trigger` | Revoke the token. Previously accepted Tasks keep their place in the queue. |

Only a Git container Application with a named branch and no explicit commit
pin can enable a trigger. The trigger binds to the saved Git source, including
its repository and build inputs. Changing that source requires a new trigger.
Manual update review and rebuild remain available when a trigger is enabled.

A CI job sends `POST /deploy/{id}` using the trigger token in
`Authorization: Bearer <token>`. The token cannot list Applications, change
configuration or deploy a different Application.

```json
{"delivery_id":"build-42","branch":"main","revision":"<full Git commit id>"}
```

`revision` is optional. The worker resolves the actual `refs/heads/main` and
pins that commit before building. When `revision` is supplied, it must still
match the branch head at execution time. A moved branch fails that Task before
building. Send a new delivery for the new head.

Invalid credentials return `401`. An authenticated event for another branch
returns `200` with `ignored: true`. An accepted event returns `202` with its
`task_id`. Replaying its delivery ID returns the same Task and
`duplicate: true`, including after a restart or failed Task. Reusing that ID
with a different payload returns `409`. Retry a failed operation with a new
ID. IDs are scoped to the Application and token generation.

The delivery receipt and pending Task share one SQLite transaction. Receipts
remain after Task retirement and audit retention. Tokens are stored as hashes;
request tokens never enter receipts, Tasks or audit events.

## Readiness

Container deployment Tasks wait up to 120 seconds for declared Docker health
checks. A stopped or unhealthy container fails the candidate. A check that
keeps starting fails at the deadline. The Application and deployment history
show the failure, and the Task ends as failed.

Restart with pull uses the same health gate and records a deployment when it
recreates registry images. A failed pull leaves the existing workload running.
A restart without pull keeps its existing behavior and creates no deployment.

The Application API has a separate `readiness` field:

| Value | Meaning |
| --- | --- |
| `checking` | Containers or declared checks have not settled. |
| `ready` | Every service is running and has a passing health check. |
| `unknown` | At least one running service has no health check, or the Application is stopped. |
| `failed` | The deployment or an observed service failed. |

A running Application without health checks can complete deployment, but the
console calls its health unverified. HTTP response status remains a separate
observation. Add a Dockerfile or Compose health check to gate deployment on
Application behavior, including any database dependency it needs.

Tasks for the same Application run one at a time, including manual deploys,
triggered builds and recovery. A restart fails interrupted checks; it never
turns an interrupted candidate into a successful deployment.

## Explicit recovery

`GET /apps/id/{id}/deployments` lists deployment outcomes and immutable image
IDs. Each successful container deployment records the images Docker actually
ran, including every Compose service and any Git revision. Snapshot definitions
remain in private Platform State. The API returns metadata only.

From Deployments, choose a successful deployment and select Recover this
deployment. The confirmation names the version and explains the data limits.
The API equivalent is `POST /apps/id/{id}/deployments/{deployment}/restore`.
Recovery queues a Task and uses local immutable images without rebuilding or
pulling. A missing image fails before changing the current Application.
External image pruning can make an old deployment unavailable.

Recovery keeps current Variables, Hostnames, route rules, network grants and
data. It restores the recorded code and Compose definition. A stopped
Application stays stopped. It does not reverse database migrations, restore
files or restore database contents. Older code must be compatible with the
current data.

Container replacement can interrupt requests. A failed candidate is left
visible for the Operator to recover explicitly; the Platform does not promise
automatic rollback or zero downtime. Accept this availability policy during
the release rehearsal before a live migration.
