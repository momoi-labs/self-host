# Source builds come from a Git repository, and local PostgreSQL is an unpublished Application

Git source builds are implemented in D2. Managed PostgreSQL remains a P2
proposal. The Platform still accepts the existing Host `path` source and
refuses `build:` in pasted Compose files (ADR-0015). Git Compose inputs come
from an owned checkout. Applications that need a database can continue to
run one inside their own Compose file.

## A source build is a repository, a ref and a pinned commit

A source build names a Git repository, a ref (a branch or a tag), a build
context (a directory inside the checkout) and its build inputs: a declared
Dockerfile path, or the Compose build inputs `build.context`,
`build.dockerfile` and `build.args`. The Platform clones the repository into
a directory it owns, records the commit it built, and materializes the build
inputs from the checkout. Nothing is read from the Host's filesystem outside
that directory. A redeploy builds the pinned commit again unless the Operator
asks for the ref's newer commit, which is then a visible change with a
before-and-after, the way a pulled image is. This is slice D2.

## Local PostgreSQL is an Application the Platform manages

Local PostgreSQL is one standalone Application: container Runtime,
`unpublished` Publication (ADR-0028), managed by the Platform, which chooses
its image, its volume and its credentials. Other Applications reach it over
the Application network through a connection reference the Platform writes
into a Variable of the consumer. It is neither Platform Infra, which ADR-0018
and ADR-0019 removed, nor a service inside each Application's Compose file.
This is slice P2.

## Considered options

- **Keep building from Host paths.** Rejected as the shape to grow: the
  Operator points at a directory and the Platform reads whatever is there,
  with no record of what was built.
- **Let `build:` in a Compose file read the Host.** Rejected: ADR-0015 refuses
  it by name, and this keeps that refusal. The build inputs come from the
  checkout instead.
- **PostgreSQL inside each Application's Compose file.** Works today and
  stays allowed. Rejected as the managed shape: one database per
  Application means one upgrade path and one backup per Application, and
  none of them the Platform's.
- **PostgreSQL as Platform Infra.** Rejected: the Platform runs nothing of its
  own in a container any more, and a database for Applications is not the
  Platform's state (ADR-0027).

**Status:** Git sources accepted and implemented in D2. Managed PostgreSQL proposed.

**Extends:** [ADR-0015](0015-compose-service-ownership-and-routing.md) (the
`build:` refusal), [ADR-0028](0028-runtime-and-publication-are-recorded-explicitly.md)
(an unpublished Application).
**Documented in:** [deployment-contracts.md](../deployment-contracts.md).
