# A development Application converts to Compose once, and does not convert back

A development Application keeps five settings: the saved image, its deployed
tag, the start command, the web port and whether `/data` persists. The Compose
file the Platform runs is generated from them on every save and stored only so
the deploy path has a file to run. `DevelopmentApplication` is the record;
`compose` is its output.

Operators will need settings the form does not have: environment variables, a
second mount, a memory limit. This decides how those arrive without two
editors writing the same field.

## The decision

A development Application shows its generated file, read only. One explicit
action, "Convert to Compose file", turns it into an ordinary Compose
Application: the Platform clears `development`, keeps the generated text as
the file's first version, and hands ownership to the Operator. From then on
nothing regenerates it.

The conversion is a one-way door. There is no action that turns a Compose
Application back into a development one.

## Why the file cannot have two owners

`development_compose` runs on every save. An Operator who edited the stored
file directly would lose that edit the next time they changed the port, with
no warning and no diff, because the form's save does not read the file it
replaces. Making the form merge into a hand-edited file instead means reading
the form's five settings back out of arbitrary YAML, and
[#100](https://github.com/momoi-labs/self-host/issues/100) rules that out for
the same reason it is a bad idea in general: a file that declares three
services, or names its web service something else, or sets `command` as a
string rather than a list, has no single correct reading.

So the file is either generated or owned, never both. A mode, not a merge.

## Why it does not convert back

Going back would have to invent the form's settings from the file. The
Applications that convert are exactly the ones the form cannot describe:
extra environment, extra volumes, resource limits. Returning them to the form
would mean dropping whatever the Operator added to get there, which is worse
than refusing.

The console says this before converting, not after. The dialog names what
stops working: the image selector, the "a newer build is available" notice,
and the start-command and persistence fields. The image association goes with
them. An Operator who rebuilds their development image afterwards pastes the
new tag into the file, the same as any other Compose Application.

## What stays out of scope

Advanced Compose editing is not a reason to grow the form. If environment
variables turn out to be what most development Applications need, they belong
in the form as a named field with its own validation, not behind a conversion.
The conversion exists for what the form should not model at all.

Converting does not restart the Application on its own. It is a save like any
other: the file it writes is the file that was already running, so only a
later edit recreates anything.

## What implementing it requires

The update API must be able to say "stop being a development Application",
which `Option<DevelopmentApplication>` cannot express today: omitting the
field means "unchanged", and `prepare_update` falls back to the current
settings. The conversion carries the file it is converting to, so the request
names the action rather than relying on a null.

`prepare_update` also refuses `compose` alongside `development`. That refusal
stays; the converting request is the one case that sends a file for an
Application that had development settings, and it sends them as gone.

Validation has to cover what the conversion is for. An Application with custom
environment, an extra volume and `deploy.resources.limits` must survive two
consecutive saves, a container restart and a Platform restart with its file
unchanged, and its data must still be where it was before the conversion.
