# A custom image recipe can hand its Dockerfile to the Operator

A custom image is built from a recipe: mise dependencies, setup commands
and build checks. The Host renders those into a Dockerfile and builds it.
Every image that mise plus a few `RUN` lines can describe is covered.

The rest are not. An image that needs a system package, a second stage, a
`COPY` from the context or a different base has nothing in the form to say so.
This decides how those images get built without turning the form into a
Dockerfile editor one field at a time.

## The decision

A recipe has two modes, and `dockerfile: Option<String>` is the switch.

In builder mode the recipe holds `dependencies`, `setup` and `build_checks`,
and the Host renders the file. In manual mode the recipe holds the file, and
the Host runs `docker build` on that text as is: no injected user, no
entrypoint, no build checks, no mise. The other three fields must arrive
empty; a request that sends both is refused rather than half applied.

One action moves between them. "Edit Dockerfile" asks the Host to render the
current fields (`POST /custom-images/dockerfile`) and puts the answer in the
editor. "Back to builder" discards the text after a confirmation and the
fields render again, unchanged.

## Why the file cannot have two owners

This is [ADR-0021](0021-development-applications-convert-to-compose-once.md)
again, one level down. Regenerating over an edited file loses the edit with no
warning, and merging an edit back into the form means reading dependencies,
setup and checks out of arbitrary Dockerfile text: `RUN` lines that install
three tools at once, a heredoc, a second stage that copies from the first.
There is no single correct reading, so there is no merge.

Unlike an Application converting to Compose, this door swings both ways. The
builder's fields survive the takeover untouched, so going back is regenerating
from what is still there, not inventing it. What does not survive is the text,
and the confirmation says so.

## What the Operator owns in manual mode

Everything. The image still has to start as a container the Platform can run,
which means keeping the `dev` user, the entrypoint and its `ENTRYPOINT` line.
The editor carries that warning permanently, and it is the only protection:
validation checks the size, the absence of stray control characters and the
presence of a `FROM`, and nothing else.

That is the same trust the Platform already extends to setup commands and to
Compose files. The Operator is the only user, on their own Host.

## Why the mise config moved into the Dockerfile

Builder mode used to write `mise.toml` beside the Dockerfile and `COPY` it. A
file handed to an Operator cannot reference a file they cannot see, so the
config now travels inside the Dockerfile as a heredoc with a quoted delimiter.
The rendered text is self-contained: the dependencies are in the file being
edited. The support scripts, which manual mode is free to drop, stay as
`COPY` from the build context in both modes.

## What this costs once

The image tag hashes the rendered Dockerfile instead of `mise.toml`, because
setup commands and hand edits have to trigger a rebuild the same way a
dependency does. Every existing image therefore hashes to a new tag on its
next save and the Host rebuilds it once. Applications keep their deployed tag
until they are redeployed, as they already do after any image edit.

## What stays out of scope

Regenerating an edited file and showing a diff. Merging hand edits back into
the builder. Build secrets. Dockerfile syntax highlighting past the bash
grammar the editor already loads.
