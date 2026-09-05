# Use Compose for MVP Applications and s6 for future native Applications

The existing single-container Application definition cannot express the runtime
configuration needed by the first real use case. Accept Docker Compose through
the web console, with the parameters needed to run the Application. Hermes is
the first validation case, using its upstream Compose example as a reference.
Reuse Compose's service, command, environment, network and storage model instead
of designing a new container manifest.

After the MVP, use s6 to run and supervise Applications without containers.
Do not install s6 on the Host in this slice or define a universal manifest for
both execution paths. The Platform itself uses launchd, as recorded in
[ADR-0013](0013-macos-bootstrap-with-launchd.md).

## Consequences

- One Application can contain multiple Compose services. Preserve the stable
  Application identity; the old one-container naming assumption does not define
  Compose service ownership. The implementation must specify that mapping.
- Support the ports and persistent mounts needed for the first Application.
  The old fixed HTTP port 80 assumption is insufficient. Document the supported
  Compose subset and its LAN exposure behavior.
- Keep Applications isolated from Platform Infra as required by
  [ADR-0012](0012-system-and-application-networks-are-separate.md). Compose support
  does not grant Applications access to the Platform's state store.
- Installing a container is not the complete user outcome. Provider credentials,
  browser configuration, persistence and restart recovery belong in acceptance
  checks for the first slice.

**Status:** accepted for implementation; s6 is deferred until after the MVP

**Partially supersedes:** the CLI-only and single-container MVP restrictions in
[ADR-0001](0001-platform-binary-docker-grpc-lan.md), the blanket exclusion of
Application port publishing in [ADR-0006](0006-lan-ports-http-dns-grpc.md), and
the one-container-per-Application naming assumption in
[ADR-0008](0008-container-naming-and-application-ids.md) for Compose Applications.
Existing Application IDs and Platform Infra isolation remain in effect.
