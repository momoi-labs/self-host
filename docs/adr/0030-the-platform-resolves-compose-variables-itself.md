# The Platform resolves Compose Variables itself

A Compose file may say `${VAR}`. Docker Compose fills that in from the
process environment and from a `.env` file beside the project, and until now
the Platform let it: the rendered project went to `docker compose` as text,
and every Application Variable was copied into every service's `environment`
on top. Two things were wrong with that. When the Operator had not set a
Variable of that name, `${VAR}` read the daemon's own environment, whatever
launchd or a shell had handed it. And every service of an Application
received every secret, whether it needed one or not.

The Platform now reads the file itself. `${VAR}` and the Compose forms
(`:-`, `-`, `:?`, `?`, `:+`, `+`, `$$`) are resolved against the
Application's Variables and nothing else; an unset name reads as empty, as
Compose does, minus the warning. The rendered project is written with every
`$` doubled, so `docker compose` finds nothing left to interpolate and the
daemon's environment has no road into a container. `<<` merge keys and `x-`
extension fields are handled while reading, so a file written for Compose
with anchors still works.

A Variable reaches a service by reference: where the file names it, and
nowhere else. Applications created before this keep broadcast delivery, every
Variable in every service, because taking it away on upgrade would break what
they rely on without a word. The Operator switches one with
`"variable_delivery": "referenced"` once the references are in the file.

## Considered options

- Let `docker compose` interpolate, handing it the Variables through
  `--env-file`. Rejected: Compose falls back to the process environment for a
  name the env file lacks, so the daemon's environment still leaks, and a
  `.env` in the project directory would be read as well.
- Keep broadcast for every Application. Rejected: one service with a shell
  and a hostile image reads every secret the Application has. Referenced is
  the smaller default.
- Refuse `$` outside `$$`. Rejected: the files Operators paste come from
  upstream READMEs that use `${VAR:-default}`, and refusing them helps
  nobody.

## Consequences

- A `:?` requirement is checked when the Application is about to run, not
  when the file is saved, so the Operator can save first and set Variables
  after. `image: ${IMAGE}` with no default cannot be checked without the
  Variable and reads as a missing image until it is set.
- A `$` meant for the container's shell, as in `sh -c 'echo $HOME'`, must be
  written `$$HOME`, exactly as in a Compose file run by hand. Any other lone
  `$` is refused by name.
- Old rows keep broadcast until the Operator switches them. Nothing they
  relied on changes on upgrade.

**Status:** accepted

**Documented in:** [compose-applications.md](../compose-applications.md).
