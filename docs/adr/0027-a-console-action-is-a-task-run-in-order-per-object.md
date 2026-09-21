# A console action is a task the scheduler runs in order per object

The console has six actions on an Application, a Virtual machine or a custom
image: create, delete, stop, start, restart and configure. Before this
decision, some ran inside the request and some on a detached `tokio::spawn`,
and the event history (#114) could say that a request was accepted or that
work had finished, but not whether work was waiting or under way. Two
requests for the same machine were refused with `409` while one ran; two
image builds could not run at once; and a stop that took a while held the
request open. [#113](https://github.com/momoi-labs/self-host/issues/113)
asked for one scheduler behind all six.

## Decision

Every one of the six actions is a task. The handler validates the request,
writes what the record needs so the screen reacts at once, persists the task
under `tasks_v1`, and answers `202 Accepted` with the task id. A delete
answers `202 {"task_id": ...}` where it answered `204` before; start, stop
and restart answer `202` with the Application as it stands and the task,
where they answered `200` with the Application as it ended up. A deploy that
is only a rename still finishes inline and answers `200`. Reads, request
validation and the edit of a machine's draft (`PUT /environments/{id}`) stay
synchronous.

The task's id is the audit event's id. The event is `pending` when the
request is accepted, `running` when a worker starts the task, and
`completed` or `failed` when the work is done. It never moves backwards. A
failed event carries `error`, the same `{error, caused_by}` shape the API
answers errors in. The object's id and name, the action, the API key that
asked and the timestamps stay on the event through every transition.

Tasks on one object run one at a time, oldest first. Objects do not wait on
each other: a long deploy on one Application delays nothing on another, and
two custom images build side by side. The `409` refusals that mirrored the
old rule ("an operation is already running", "an image is already building")
are gone; the queue orders the work instead. Refusals that are about the
request itself stay: a name that is taken, an image in use, a delete without
its confirmation.

On a restart the daemon settles the queue before it serves. A `pending` task
did nothing yet, so it goes back on its queue in the order it arrived, with
its id and actor; its persisted payload is what makes that possible. A
`running` task was mid-way through a Docker or Lima operation nobody can
resume, so it fails with "The Platform restarted before this task finished."
and its object's own recovery says what is left: a machine reads
`interrupted`, an image `failed`, an Application whatever Docker is running.
A `running` event with no task behind it, written by a daemon that had no
queue yet, fails the same way. Nothing is retried on its own; the Operator
asks again, which is a new task.

## What this changes for the console

`GET /events` answers the four states above and nothing else. The mapping the
Events page kept for the legacy `accepted` status is gone; a legacy
acceptance record that no completion was ever matched to reads as `pending`
and is never run. A screen that starts an action follows its task by id until
the event is terminal, then reports the outcome from the event; the
Application page does this for start, stop, restart and delete, the images
page for delete. Machines and deploys already polled their record and keep
doing so.

## Consequences

The history is rewritten on every transition, as it was, plus one write per
task for `running`; the queue file is rewritten as tasks come and go. That
is a few kilobytes per action on a home lab. A rotation is not part of this
decision.

The environment value a queued `configure` carries sits in `tasks_v1` until
the task runs. It is the same Platform State directory that already holds
every Application's environment, and the audit history never sees it.

A second action on a machine no longer answers `409`; it answers `202` and
waits. The machine's record shows the operation that is executing, and the
Events page shows the one waiting. The record moves to the waiting action
when its turn comes.

**Status:** accepted
**Context:** [#113](https://github.com/momoi-labs/self-host/issues/113),
after the event history of [#114](https://github.com/momoi-labs/self-host/pull/114).
