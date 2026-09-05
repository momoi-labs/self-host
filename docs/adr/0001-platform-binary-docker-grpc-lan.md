# Platform = binary + Docker on LAN; control via API

Primary environment is a LAN Host, without depending on external SaaS on the happy path. The Platform is a single binary (daemon) that starts Infra and Applications as Docker containers; the Operator controls it via a CLI talking to the daemon API. UI, Kubernetes, and git-as-source are out of MVP — MVP Deploy is image pull or local build.

The Operator API protocol was initially gRPC; that is **superseded by ADR-0007** (HTTP JSON).

**Status:** accepted

The next MVP's Host and Bootstrap requirements are amended by
[ADR-0013](0013-macos-bootstrap-with-launchd.md). Its CLI-only and single-container
restrictions are superseded by
[ADR-0014](0014-compose-applications-and-future-native-supervision.md).
