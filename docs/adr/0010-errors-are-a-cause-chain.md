# An error is a failure plus its causes, never one flattened sentence

Every error in the Platform keeps its layers apart. A `Display` impl states
only what *that* layer failed to do and delegates the rest to `source()`;
nothing interpolates `{e}` into its own message. At the edges — the terminal,
the HTTP body, the stored `last_error` — the chain is flattened once, into a
failure and the causes underneath it.

The API shape is that flattening:

```json
{ "error": "failed to deploy the Application",
  "caused_by": ["failed to pull image 'b'",
                "Error response from daemon",
                "pull access denied for b",
                "denied",
                "requested access to the resource is denied"] }
```

The CLI reports the same thing as an `anyhow` chain:

```
Error: Deploy failed (500 Internal Server Error)

Caused by:
    0: failed to deploy the Application
    1: failed to pull image 'b'
    2: Error response from daemon
    3: pull access denied for b
    4: denied
    5: requested access to the resource is denied
```

Errors used to glue themselves together — `DeployError::Docker(e)` wrote
`{e}`, which wrote `{e}` again — and arrived as one unreadable line where the
Operator could not tell the Platform's framing apart from what Docker said.
Every layer was present and none of them was legible.

## Considered options

- **Keep the flattened string and format it for the terminal.** Rejected: the
  layers are already gone by the time anyone reads it, and splitting a sentence
  back apart is guesswork.
- **`anyhow` end to end, typed errors dropped.** Rejected: the HTTP handlers
  match on the variant to pick a status code, and `anyhow` erases exactly that.
- **Typed errors in the library, `anyhow` in the binary.** Chosen. The library
  keeps `source()` honest; the CLI gets chain rendering for free and rebuilds a
  chain from an API response so a failure on the Host prints like a local one.

## Consequences

`last_error` is a `JSONB` column holding the report, not a `TEXT` sentence. A
deploy fails on a task, long after the request was answered, so the chain has
to survive in the database or the console never sees it. Existing rows migrate
to a failure with an empty `caused_by`.

The console renders the report directly: the alert's title is the failure, its
body is the numbered chain. A toast keeps naming the Application in its title,
because it shows up detached from whatever raised it.

A `docker` command that is refused is not Docker being unavailable, and no
longer says so. `DockerError::Command` names the step we asked for — "failed to
pull image 'b'" — and Docker's own answer goes underneath, in Docker's words.

That answer is itself a chain. Docker is written in Go, and Go wraps an error
by writing `outer: inner`, so a refusal arrives as layers flattened onto one
line. `DockerRefusal::parse` takes them apart again on `": "` — a colon *and* a
space, which is what makes it safe: `nginx:1.2.3` and `1.2.3.4:443` have no
space after the colon and come through untouched. Output spanning several lines
is a build log rather than a wrapped error, and is left alone.

This is the one place the Platform reads another tool's formatting, and it is
worth the exception: the alternative is one 160-character line where the
registry, the daemon and the CLI all talk at once.

A layer that adds no framing of its own now says so out loud rather than
echoing its cause. `DockerError::Compose` reads "the Docker Compose command
failed" — worth a line, since the cause below it is `docker compose` talking.

**Status:** accepted
